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
