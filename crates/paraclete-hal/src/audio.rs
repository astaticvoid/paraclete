use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, SampleRate, StreamConfig};

use paraclete_runtime::NodeExecutor;

pub struct AudioBackend {
    _stream: cpal::Stream,
}

impl AudioBackend {
    /// Open the default output device and start streaming.
    ///
    /// `executor` is moved into the audio callback and drives the graph.
    /// The returned `AudioBackend` keeps the stream alive — drop it to stop audio.
    pub fn start(mut executor: NodeExecutor) -> Result<Self, AudioError> {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .ok_or(AudioError::NoOutputDevice)?;

        log::info!("audio device: {}", device.name().unwrap_or_default());

        let supported = device
            .default_output_config()
            .map_err(|e| AudioError::Config(e.to_string()))?;

        log::info!(
            "default config: {:?} Hz, {} ch, {:?}",
            supported.sample_rate(),
            supported.channels(),
            supported.sample_format()
        );

        // Request f32 samples. cpal will resample / convert if needed.
        let channels = supported.channels() as usize;
        let sample_rate = supported.sample_rate().0;

        let config = StreamConfig {
            channels: supported.channels(),
            sample_rate: SampleRate(sample_rate),
            buffer_size: BufferSize::Default,
        };

        let err_fn = |err| log::error!("audio stream error: {err}");

        let stream = match supported.sample_format() {
            SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    executor.process(data, channels);
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => {
                let mut f32_buf: Vec<f32> = Vec::new();
                device.build_output_stream(
                    &config,
                    move |data: &mut [i16], _| {
                        if f32_buf.len() != data.len() {
                            f32_buf.resize(data.len(), 0.0);
                        }
                        executor.process(&mut f32_buf, channels);
                        for (dst, &src) in data.iter_mut().zip(f32_buf.iter()) {
                            *dst = (src * i16::MAX as f32) as i16;
                        }
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::U16 => {
                let mut f32_buf: Vec<f32> = Vec::new();
                device.build_output_stream(
                    &config,
                    move |data: &mut [u16], _| {
                        if f32_buf.len() != data.len() {
                            f32_buf.resize(data.len(), 0.0);
                        }
                        executor.process(&mut f32_buf, channels);
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
    ) -> Result<Self, AudioError> {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .ok_or(AudioError::NoOutputDevice)?;

        log::info!("audio device (engine): {}", device.name().unwrap_or_default());

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

        let config = StreamConfig {
            channels: supported.channels(),
            sample_rate: SampleRate(sample_rate),
            buffer_size: BufferSize::Default,
        };

        let err_fn = |err| log::error!("audio stream error: {err}");

        // Macro to build the callback for a given sample type via an f32 intermediate.
        // The closure captures the three Arcs and converts samples after processing.
        macro_rules! build_f32_callback {
            ($data_type:ty, $convert:expr) => {{
                let exec_cell = executor_cell.clone();
                let pause_req = pause_requested.clone();
                let is_pau    = is_paused.clone();
                let mut f32_buf: Vec<f32> = Vec::new();
                move |data: &mut [$data_type], _| {
                    if pause_req.load(Ordering::Acquire) {
                        data.fill($convert(0.0f32));
                        is_pau.store(true, Ordering::Release);
                        return;
                    }
                    is_pau.store(false, Ordering::Release);
                    if f32_buf.len() != data.len() {
                        f32_buf.resize(data.len(), 0.0);
                    }
                    if let Ok(mut guard) = exec_cell.try_lock() {
                        if let Some(exec) = guard.as_mut() {
                            exec.process(&mut f32_buf, channels);
                        } else {
                            f32_buf.fill(0.0);
                        }
                    } else {
                        f32_buf.fill(0.0);
                    }
                    for (dst, &src) in data.iter_mut().zip(f32_buf.iter()) {
                        *dst = $convert(src);
                    }
                }
            }};
        }

        let stream = match supported.sample_format() {
            SampleFormat::F32 => {
                let exec_cell = executor_cell.clone();
                let pause_req = pause_requested.clone();
                let is_pau    = is_paused.clone();
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
                                exec.process(data, channels);
                            } else {
                                data.fill(0.0);
                            }
                        } else {
                            data.fill(0.0);
                        }
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::I16 => {
                device.build_output_stream(
                    &config,
                    build_f32_callback!(i16, |s: f32| (s * i16::MAX as f32) as i16),
                    err_fn,
                    None,
                )
            }
            SampleFormat::U16 => {
                device.build_output_stream(
                    &config,
                    build_f32_callback!(u16, |s: f32| ((s + 1.0) * 0.5 * u16::MAX as f32) as u16),
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

/// Audio engine with dynamic topology support (ADR-029).
///
/// Wraps the audio backend with a pause/resume protocol that allows
/// `apply_patch()` to swap the `NodeExecutor` at runtime.
pub struct AudioEngine {
    executor_cell:   Arc<Mutex<Option<NodeExecutor>>>,
    pause_requested: Arc<AtomicBool>,
    is_paused:       Arc<AtomicBool>,
    _backend:        Option<AudioBackend>,
}

impl AudioEngine {
    /// Create an `AudioEngine` in the paused state with no executor.
    /// Useful for tests and before audio is started.
    pub fn new_paused() -> Self {
        AudioEngine {
            executor_cell:   Arc::new(Mutex::new(None)),
            pause_requested: Arc::new(AtomicBool::new(true)),
            is_paused:       Arc::new(AtomicBool::new(true)),
            _backend:        None,
        }
    }

    /// Start audio with the given executor.
    pub fn start(executor: NodeExecutor) -> Result<Self, AudioError> {
        let executor_cell    = Arc::new(Mutex::new(Some(executor)));
        let pause_requested  = Arc::new(AtomicBool::new(false));
        let is_paused        = Arc::new(AtomicBool::new(false));

        let backend = AudioBackend::start_with_callback(
            executor_cell.clone(),
            pause_requested.clone(),
            is_paused.clone(),
        )?;

        Ok(AudioEngine {
            executor_cell,
            pause_requested,
            is_paused,
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while !self.is_paused.load(Ordering::Acquire) {
            if std::time::Instant::now() > deadline {
                panic!("AudioEngine::wait_paused: audio thread did not pause within 500 ms");
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
    pub fn resume_with_executor(&self, executor: NodeExecutor) {
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
