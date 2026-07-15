use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, SampleRate, StreamConfig};

use paraclete_runtime::{NodeExecutor, RuntimeCounters};

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
            buffer_size: BufferSize::Default,
        };

        let err_fn = |err| log::error!("audio stream error: {err}");

        // Pre-allocate a work buffer for the partial-chunk fallback path.
        // Allocated on the main thread before the stream starts.
        let work_buf: Vec<f32> = vec![0.0; block_samples];

        let stream = match supported.sample_format() {
            SampleFormat::F32 => {
                let mut work_buf = work_buf;
                device.build_output_stream(
                    &config,
                    move |data: &mut [f32], _| {
                        audio_callback_f32(
                            data,
                            &mut work_buf,
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
            buffer_size: BufferSize::Default,
        };

        let err_fn = |err| log::error!("audio stream error: {err}");
        let ct = counters.clone();
        let work_buf: Vec<f32> = vec![0.0; block_samples];

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

        Ok(Self { _stream: stream })
    }
}

/// Process the f32 output buffer through the executor.
///
/// When `data.len() == block_samples` the executor is called directly (fast
/// path, zero overhead).  Otherwise the buffer is processed in full-block
/// chunks: the executor is called once per chunk; any partial final chunk is
/// filled from the first `chunk.len()` frames of a full-block render.
#[inline]
fn audio_callback_f32(
    data: &mut [f32],
    work_buf: &mut [f32],
    executor: &mut NodeExecutor,
    channels: usize,
    block_samples: usize,
) {
    if data.len() == block_samples {
        executor.process(data, channels);
        return;
    }

    for chunk in data.chunks_mut(block_samples) {
        if chunk.len() == block_samples {
            executor.process(chunk, channels);
        } else {
            work_buf.fill(0.0);
            executor.process(work_buf, channels);
            chunk.copy_from_slice(&work_buf[..chunk.len()]);
        }
    }
}

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
