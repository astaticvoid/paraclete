use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, SampleRate, StreamConfig};

use paraclete_runtime::{NodeExecutor, RuntimeCounters};

#[cfg(target_os = "linux")]
mod lrt {
    use std::sync::atomic::{AtomicBool, Ordering};

    #[repr(C)]
    struct SchedParam {
        sched_priority: i32,
    }

    const SCHED_FIFO: i32 = 1;

    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_setschedparam(
            thread: usize,
            policy: i32,
            param: *const SchedParam,
        ) -> i32;
    }

    static ONCE: AtomicBool = AtomicBool::new(false);

    pub fn try_set_realtime() {
        if ONCE.swap(true, Ordering::Relaxed) {
            return;
        }
        let param = SchedParam { sched_priority: 50 };
        let ret = unsafe { pthread_setschedparam(pthread_self(), SCHED_FIFO, &param) };
        if ret != 0 {
            log::warn!("realtime priority: pthread_setschedparam SCHED_FIFO failed (errno={})", ret);
        } else {
            log::info!("realtime priority: set SCHED_FIFO prio=50 on audio thread");
        }
    }
}

pub struct AudioBackend {
    _stream: cpal::Stream,
}

/// Query the default output device's sample rate without opening a stream.
/// Returns `None` if no output device is available or the config is inaccessible.
pub fn query_sample_rate() -> Option<f32> {
    let host = cpal::default_host();
    let device = host.default_output_device()?;
    device
        .default_output_config()
        .map(|c| c.sample_rate().0 as f32)
        .ok()
}

impl AudioBackend {
    /// Open the default output device and start streaming.
    ///
    /// `executor` is moved into the audio callback and drives the graph.
    /// The returned `AudioBackend` keeps the stream alive — drop it to stop audio.
    ///
    /// When the device buffer size equals `block_size * channels` (the common
    /// case), the executor is called directly with zero overhead.  When the
    /// sizes differ the callback chunk-processes: full blocks are rendered and
    /// a partial final chunk uses the first N frames of a full block (the
    /// clock advances by a full block — acceptable transient for
    /// non-power-of-two device buffers; the long-run cadence is correct on
    /// the common path).
    pub fn start(mut executor: NodeExecutor) -> Result<Self, AudioError> {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .ok_or(AudioError::NoOutputDevice)?;

        log::info!(
            "audio device: {}",
            device.name().unwrap_or_default()
        );

        let supported = device
            .default_output_config()
            .map_err(|e| AudioError::Config(e.to_string()))?;

        log::info!(
            "default config: {:?} Hz, {} ch, {:?}",
            supported.sample_rate(),
            supported.channels(),
            supported.sample_format()
        );

        let channels = supported.channels() as usize;
        let sample_rate = supported.sample_rate().0;
        let block_size = executor.block_size();
        let block_samples = block_size * channels;

        let config = StreamConfig {
            channels: supported.channels(),
            sample_rate: SampleRate(sample_rate),
            // Use Default buffer size — the output ring bridge handles any
            // mismatch between the device buffer and internal block size.
            // Fixed would skip the ring for exact-match hardware but some
            // ALSA devices reject it at open time even when their reported
            // config ranges include the size.
            buffer_size: BufferSize::Default,
        };

        let err_fn = |err| log::error!("audio stream error: {err}");

        // Pre-allocate a work buffer and output ring for the callback.
        // Allocated on the main thread before the stream starts; never
        // allocates on the audio path.
        let work_buf: Vec<f32> = vec![0.0; block_samples];
        let mut ring = OutputRing::new(block_samples, RING_BLOCKS);

        let stream = match supported.sample_format() {
            SampleFormat::F32 => {
                let mut work_buf = work_buf;
                device.build_output_stream(
                    &config,
                    move |data: &mut [f32], _| {
                        audio_callback_f32(
                            data,
                            &mut work_buf,
                            &mut ring,
                            &mut executor,
                            channels,
                            block_samples,
                        );
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::I16 => {
                let mut work_buf = work_buf;
                let mut f32_buf: Vec<f32> = Vec::new();
                device.build_output_stream(
                    &config,
                    move |data: &mut [i16], _| {
                        if f32_buf.len() != data.len() {
                            f32_buf.resize(data.len(), 0.0);
                        }
                        audio_callback_f32(
                            &mut f32_buf,
                            &mut work_buf,
                            &mut ring,
                            &mut executor,
                            channels,
                            block_samples,
                        );
                        for (dst, &src) in data.iter_mut().zip(f32_buf.iter()) {
                            *dst = (src * i16::MAX as f32) as i16;
                        }
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::U16 => {
                let mut work_buf = work_buf;
                let mut f32_buf: Vec<f32> = Vec::new();
                device.build_output_stream(
                    &config,
                    move |data: &mut [u16], _| {
                        if f32_buf.len() != data.len() {
                            f32_buf.resize(data.len(), 0.0);
                        }
                        audio_callback_f32(
                            &mut f32_buf,
                            &mut work_buf,
                            &mut ring,
                            &mut executor,
                            channels,
                            block_samples,
                        );
                        for (dst, &src) in data.iter_mut().zip(f32_buf.iter()) {
                            *dst = ((src + 1.0) * 0.5 * u16::MAX as f32) as u16;
                        }
                    },
                    err_fn,
                    None,
                )
            }
            _ => return Err(AudioError::Config("unsupported sample format".into())),
        }
        .map_err(|e| AudioError::Build(e.to_string()))?;

        stream.play().map_err(|e| AudioError::Play(e.to_string()))?;

        log::info!("audio stream started");
        enable_ftz_daz();

        Ok(Self { _stream: stream })
    }

    /// Start audio with shared executor cell and pause/resume atomics.
    /// Used by `AudioEngine` to support dynamic topology (ADR-029).
    pub(crate) fn start_with_callback(
        executor_cell: Arc<Mutex<Option<NodeExecutor>>>,
        pause_requested: Arc<AtomicBool>,
        is_paused: Arc<AtomicBool>,
        counters: Arc<RuntimeCounters>,
    ) -> Result<Self, AudioError> {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .ok_or(AudioError::NoOutputDevice)?;

        log::info!(
            "audio device (engine): {}",
            device.name().unwrap_or_default()
        );

        let supported = device
            .default_output_config()
            .map_err(|e| AudioError::Config(e.to_string()))?;

        log::info!(
            "default config: {:?} Hz, {} ch, {:?}",
            supported.sample_rate(),
            supported.channels(),
            supported.sample_format()
        );

        let channels = supported.channels() as usize;
        let sample_rate = supported.sample_rate().0;

        let block_size = executor_cell
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|e| e.block_size()))
            .unwrap_or(512);
        let block_samples = block_size * channels;

        let config = StreamConfig {
            channels: supported.channels(),
            sample_rate: SampleRate(sample_rate),
            // Use Default buffer size — the output ring bridge handles any
            // mismatch between the device buffer and internal block size.
            // Fixed would skip the ring for exact-match hardware but some
            // ALSA devices reject it at open time even when their reported
            // config ranges include the size.
            buffer_size: BufferSize::Default,
        };

        let err_fn = |err| log::error!("audio stream error: {err}");
        let ct = counters.clone();
        let work_buf: Vec<f32> = vec![0.0; block_samples];
        let mut ring = OutputRing::new(block_samples, RING_BLOCKS);

        let stream = match supported.sample_format() {
            SampleFormat::F32 => {
                let exec_cell = executor_cell.clone();
                let pause_req = pause_requested.clone();
                let is_pau = is_paused.clone();
                let c = ct.clone();
                let mut work_buf = work_buf;
                device.build_output_stream(
                    &config,
                    move |data: &mut [f32], _| {
                        if pause_req.load(Ordering::Acquire) {
                            data.fill(0.0);
                            is_pau.store(true, Ordering::Release);
                            return;
                        }
                        is_pau.store(false, Ordering::Release);
                        if let Ok(mut guard) = exec_cell.try_lock() {
                            if let Some(exec) = guard.as_mut() {
                                audio_callback_f32(
                                    data,
                                    &mut work_buf,
                                    &mut ring,
                                    exec,
                                    channels,
                                    block_samples,
                                );
                            } else {
                                c.dropout_no_executor
                                    .fetch_add(1, Ordering::Relaxed);
                                data.fill(0.0);
                            }
                        } else {
                            c.dropout_lock_miss
                                .fetch_add(1, Ordering::Relaxed);
                            data.fill(0.0);
                        }
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::I16 => {
                let exec_cell = executor_cell.clone();
                let pause_req = pause_requested.clone();
                let is_pau = is_paused.clone();
                let c = ct.clone();
                let mut work_buf = work_buf;
                let mut f32_buf: Vec<f32> = Vec::new();
                device.build_output_stream(
                    &config,
                    move |data: &mut [i16], _| {
                        if pause_req.load(Ordering::Acquire) {
                            data.fill(0);
                            is_pau.store(true, Ordering::Release);
                            return;
                        }
                        is_pau.store(false, Ordering::Release);
                        if f32_buf.len() != data.len() {
                            f32_buf.resize(data.len(), 0.0);
                        }
                        if let Ok(mut guard) = exec_cell.try_lock() {
                            if let Some(exec) = guard.as_mut() {
                                audio_callback_f32(
                                    &mut f32_buf,
                                    &mut work_buf,
                                    &mut ring,
                                    exec,
                                    channels,
                                    block_samples,
                                );
                            } else {
                                c.dropout_no_executor
                                    .fetch_add(1, Ordering::Relaxed);
                                f32_buf.fill(0.0);
                            }
                        } else {
                            c.dropout_lock_miss
                                .fetch_add(1, Ordering::Relaxed);
                            f32_buf.fill(0.0);
                        }
                        for (dst, &src) in data.iter_mut().zip(f32_buf.iter()) {
                            *dst = (src * i16::MAX as f32) as i16;
                        }
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::U16 => {
                let exec_cell = executor_cell.clone();
                let pause_req = pause_requested.clone();
                let is_pau = is_paused.clone();
                let c = ct.clone();
                let mut work_buf = work_buf;
                let mut f32_buf: Vec<f32> = Vec::new();
                device.build_output_stream(
                    &config,
                    move |data: &mut [u16], _| {
                        if pause_req.load(Ordering::Acquire) {
                            data.fill(0);
                            is_pau.store(true, Ordering::Release);
                            return;
                        }
                        is_pau.store(false, Ordering::Release);
                        if f32_buf.len() != data.len() {
                            f32_buf.resize(data.len(), 0.0);
                        }
                        if let Ok(mut guard) = exec_cell.try_lock() {
                            if let Some(exec) = guard.as_mut() {
                                audio_callback_f32(
                                    &mut f32_buf,
                                    &mut work_buf,
                                    &mut ring,
                                    exec,
                                    channels,
                                    block_samples,
                                );
                            } else {
                                c.dropout_no_executor
                                    .fetch_add(1, Ordering::Relaxed);
                                f32_buf.fill(0.0);
                            }
                        } else {
                            c.dropout_lock_miss
                                .fetch_add(1, Ordering::Relaxed);
                            f32_buf.fill(0.0);
                        }
                        for (dst, &src) in data.iter_mut().zip(f32_buf.iter()) {
                            *dst = ((src + 1.0) * 0.5 * u16::MAX as f32) as u16;
                        }
                    },
                    err_fn,
                    None,
                )
            }
            _ => return Err(AudioError::Config("unsupported sample format".into())),
        }
        .map_err(|e| AudioError::Build(e.to_string()))?;

        stream.play().map_err(|e| AudioError::Play(e.to_string()))?;
        log::info!("audio engine stream started");
        enable_ftz_daz();

        Ok(Self { _stream: stream })
    }
}

/// Output ring buffer bridging the executor's fixed-block rendering with the
/// device's variable-size callback buffers.
///
/// The executor always renders `block_samples` at a time.  The ring accumulates
/// these rendered blocks and the callback drains exactly `data.len()` samples
/// each cycle.  No audio is discarded; timing error is bounded to the ring depth.
///
/// Pre-allocated on the main thread; never allocates on the audio path.
struct OutputRing {
    buf: Vec<f32>,
    /// Next position the executor will write into.
    write_pos: usize,
    /// Next position the callback will read from.
    read_pos: usize,
    /// Total ring capacity in samples.
    cap: usize,
    /// How many samples have been written (modulo cap for position).
    total_written: u64,
    /// How many samples have been read (modulo cap for position).
    total_read: u64,
}

impl OutputRing {
    fn new(block_samples: usize, num_blocks: usize) -> Self {
        let cap = block_samples * num_blocks;
        Self {
            buf: vec![0.0; cap],
            write_pos: 0,
            read_pos: 0,
            cap,
            total_written: 0,
            total_read: 0,
        }
    }

    /// How many samples are available to drain.
    fn available(&self) -> usize {
        (self.total_written - self.total_read) as usize
    }

    /// Write a full block of rendered samples into the ring.
    fn write_block(&mut self, block: &[f32]) {
        debug_assert_eq!(block.len(), self.cap / 4); // block is 1/N of ring
        let len = block.len();
        let end = self.write_pos + len;
        if end <= self.cap {
            self.buf[self.write_pos..end].copy_from_slice(block);
        } else {
            let first = self.cap - self.write_pos;
            self.buf[self.write_pos..].copy_from_slice(&block[..first]);
            self.buf[..len - first].copy_from_slice(&block[first..]);
        }
        self.write_pos = end % self.cap;
        self.total_written += len as u64;
    }

    /// Drain up to `out.len()` samples into `out`.  Returns the number of
    /// samples actually copied (may be less than `out.len()` on underrun).
    fn drain(&mut self, out: &mut [f32]) -> usize {
        let avail = self.available();
        let n = out.len().min(avail);
        if n == 0 {
            return 0;
        }
        let end = self.read_pos + n;
        if end <= self.cap {
            out[..n].copy_from_slice(&self.buf[self.read_pos..end]);
        } else {
            let first = self.cap - self.read_pos;
            out[..first].copy_from_slice(&self.buf[self.read_pos..]);
            out[first..n].copy_from_slice(&self.buf[..n - first]);
        }
        self.read_pos = end % self.cap;
        self.total_read += n as u64;
        n
    }
}

/// Process the f32 output buffer through the executor.
///
/// The executor renders fixed-size blocks into an output ring.  The callback
/// drains the ring to satisfy the device buffer.  If the ring underruns
/// (ring is empty but device wants more data), the remainder is filled with
/// silence — a self-correcting transient; the next callback picks up where
/// the ring left off.
#[inline]
fn audio_callback_f32(
    data: &mut [f32],
    work_buf: &mut [f32],
    ring: &mut OutputRing,
    executor: &mut NodeExecutor,
    channels: usize,
    block_samples: usize,
) {
    #[cfg(target_os = "linux")]
    lrt::try_set_realtime();

    // Process full blocks: render into ring, drain to output.
    let mut out_pos = 0usize;
    while out_pos < data.len() {
        let remaining = data.len() - out_pos;

        // If ring doesn't have enough, render another block into it.
        if ring.available() < block_samples {
            work_buf.fill(0.0);
            executor.process(work_buf, channels);
            ring.write_block(work_buf);
        }

        // Drain what we can (up to remaining).
        let chunk = remaining.min(block_samples);
        let got = ring.drain(&mut data[out_pos..out_pos + chunk]);
        out_pos += got;

        // Underrun guard: if we got less than requested (ring empty),
        // the executor isn't rendering. Pad with silence and break.
        if got < chunk {
            data[out_pos..].fill(0.0);
            break;
        }
    }
}

const RING_BLOCKS: usize = 4;

/// Audio engine with dynamic topology support (ADR-029).
///
/// Wraps the audio backend with a pause/resume protocol that allows
/// `apply_patch()` to swap the `NodeExecutor` at runtime.
pub struct AudioEngine {
    executor_cell: Arc<Mutex<Option<NodeExecutor>>>,
    pause_requested: Arc<AtomicBool>,
    is_paused: Arc<AtomicBool>,
    counters: Arc<RuntimeCounters>,
    _backend: Option<AudioBackend>,
}

impl AudioEngine {
    /// Create an `AudioEngine` in the paused state with no executor.
    /// Useful for tests and before audio is started.
    pub fn new_paused() -> Self {
        AudioEngine {
            executor_cell: Arc::new(Mutex::new(None)),
            pause_requested: Arc::new(AtomicBool::new(true)),
            is_paused: Arc::new(AtomicBool::new(true)),
            counters: Arc::new(RuntimeCounters::default()),
            _backend: None,
        }
    }

    /// Start audio with the given executor.
    pub fn start(mut executor: NodeExecutor) -> Result<Self, AudioError> {
        let counters = Arc::new(RuntimeCounters::default());
        executor.set_counters(counters.clone());

        let executor_cell = Arc::new(Mutex::new(Some(executor)));
        let pause_requested = Arc::new(AtomicBool::new(false));
        let is_paused = Arc::new(AtomicBool::new(false));

        let backend = AudioBackend::start_with_callback(
            executor_cell.clone(),
            pause_requested.clone(),
            is_paused.clone(),
            counters.clone(),
        )?;

        Ok(AudioEngine {
            executor_cell,
            pause_requested,
            is_paused,
            counters,
            _backend: Some(backend),
        })
    }

    /// Signal the audio thread to pause after the current buffer. Non-blocking.
    pub fn pause(&self) {
        self.pause_requested.store(true, Ordering::Release);
    }

    /// Block until the audio thread confirms it is paused.
    ///
    /// Timeout: 500 ms — panics if exceeded (indicates a hung audio thread).
    pub fn wait_paused(&self) {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(500);
        while !self.is_paused.load(Ordering::Acquire) {
            if std::time::Instant::now() > deadline {
                panic!(
                    "AudioEngine::wait_paused: audio thread did not pause within 500 ms"
                );
            }
            std::thread::yield_now();
        }
    }

    /// Take the current executor out of the engine (for `apply_patch` internal use).
    /// Returns `None` if no executor was present (e.g. `new_paused()` engine).
    pub fn take_executor(&self) -> Option<NodeExecutor> {
        self.executor_cell.lock().unwrap().take()
    }

    /// Replace the executor and resume audio processing.
    ///
    /// Must only be called after `wait_paused()` has returned.
    pub fn resume_with_executor(&self, mut executor: NodeExecutor) {
        executor.set_counters(self.counters.clone());
        *self.executor_cell.lock().unwrap() = Some(executor);
        self.is_paused.store(false, Ordering::Release);
        self.pause_requested.store(false, Ordering::Release);
    }
}

#[derive(Debug)]
pub enum AudioError {
    NoOutputDevice,
    Config(String),
    Build(String),
    Play(String),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoOutputDevice => write!(f, "no default output device found"),
            Self::Config(s) => write!(f, "device config error: {s}"),
            Self::Build(s) => write!(f, "stream build error: {s}"),
            Self::Play(s) => write!(f, "stream play error: {s}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// Enable flush-to-zero and denormals-are-zero on the calling thread.
///
/// On x86/x86-64, subnormal (denormal) floats in DSP loops trigger microcode
/// assist handling that can stall the audio thread. This sets the SSE MXCSR
/// FTZ and DAZ flags once.  Arm NEON does not require explicit denormal
/// handling (it flushes to zero by default in hardware).
fn enable_ftz_daz() {
    #[cfg(target_arch = "x86_64")]
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::Relaxed) {
            unsafe {
                // MXCSR register: bit 15 = FTZ, bit 6 = DAZ.
                let mut mxcsr: u32 = 0;
                const FLAGS: u32 = (1 << 15) | (1 << 6);
                std::arch::asm!(
                    "stmxcsr [{0}]",
                    "or dword ptr [{0}], {1}",
                    "ldmxcsr [{0}]",
                    in(reg) &mut mxcsr,
                    const FLAGS,
                );
            }
            log::info!("FTZ/DAZ enabled on audio thread");
        }
    }
}
