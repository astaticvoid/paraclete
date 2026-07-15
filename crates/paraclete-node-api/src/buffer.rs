/// Planar multi-channel audio buffer.
/// Layout: all frames for channel 0, then channel 1, etc.
/// Interleaved conversion occurs only at the HAL boundary.
pub struct AudioBuffer {
    data: Vec<f32>,
    channels: usize,
    frames: usize,
}

impl AudioBuffer {
    pub fn new(channels: usize, frames: usize) -> Self {
        Self {
            data: vec![0.0; channels * frames],
            channels,
            frames,
        }
    }

    pub fn channel(&self, index: usize) -> &[f32] {
        assert!(index < self.channels, "channel index out of bounds");
        let start = index * self.frames;
        &self.data[start..start + self.frames]
    }

    pub fn channel_mut(&mut self, index: usize) -> &mut [f32] {
        assert!(index < self.channels, "channel index out of bounds");
        let start = index * self.frames;
        &mut self.data[start..start + self.frames]
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Fill all channels with zeros.
    pub fn clear(&mut self) {
        self.data.fill(0.0);
    }

    /// Copy interleaved samples out for delivery to a hardware driver.
    /// Panics if `out.len() != channels * frames`.
    pub fn write_interleaved(&self, out: &mut [f32]) {
        assert_eq!(out.len(), self.channels * self.frames);
        for frame in 0..self.frames {
            for ch in 0..self.channels {
                out[frame * self.channels + ch] = self.data[ch * self.frames + frame];
            }
        }
    }
}

// ── Internal allocation shared by all non-audio signal types ─────────────────
// Not re-exported from lib.rs. The runtime allocates signal buffers via the
// public constructors on each typed wrapper below.

pub(crate) struct SignalBuffer {
    data: Vec<f32>,
    frames: usize,
}

impl SignalBuffer {
    pub(crate) fn new(frames: usize) -> Self {
        Self {
            data: vec![0.0; frames],
            frames,
        }
    }

    pub(crate) fn frames(&self) -> usize {
        self.frames
    }

    pub(crate) fn as_slice(&self) -> &[f32] {
        &self.data
    }

    pub(crate) fn as_slice_mut(&mut self) -> &mut [f32] {
        &mut self.data
    }
}

// ── AsSignal / AsSignalMut ────────────────────────────────────────────────────

pub trait AsSignal {
    fn frames(&self) -> usize;
    fn as_slice(&self) -> &[f32];
}

pub trait AsSignalMut: AsSignal {
    fn as_slice_mut(&mut self) -> &mut [f32];
}

// ── Typed signal wrappers ─────────────────────────────────────────────────────
// Each is a newtype around SignalBuffer.
// Deref<Target=[f32]> allows direct sample access in DSP code.

macro_rules! signal_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        pub struct $name(pub(crate) SignalBuffer);

        impl $name {
            pub fn new(frames: usize) -> Self {
                Self(SignalBuffer::new(frames))
            }

            pub fn frames(&self) -> usize {
                self.0.frames()
            }

            pub fn clear(&mut self) {
                self.0.as_slice_mut().fill(0.0);
            }
        }

        impl std::ops::Deref for $name {
            type Target = [f32];
            fn deref(&self) -> &[f32] {
                self.0.as_slice()
            }
        }

        impl std::ops::DerefMut for $name {
            fn deref_mut(&mut self) -> &mut [f32] {
                self.0.as_slice_mut()
            }
        }

        impl AsSignal for $name {
            fn frames(&self) -> usize {
                self.0.frames()
            }
            fn as_slice(&self) -> &[f32] {
                self.0.as_slice()
            }
        }

        impl AsSignalMut for $name {
            fn as_slice_mut(&mut self) -> &mut [f32] {
                self.0.as_slice_mut()
            }
        }
    };
}

signal_type!(
    CvBuffer,
    "General-purpose CV — audio rate, bipolar −1.0..+1.0."
);

signal_type!(
    PhaseBuffer,
    "Phase ramp — unipolar 0.0..< 1.0. Drives oscillators and step sequencers. \
     Values outside range are wrapped at input ports."
);

signal_type!(
    LogicBuffer,
    "Gate / trigger — high: 1.0, low: 0.0. \
     Inputs threshold at 0.5; triggers fire on the 0→1 edge only."
);

signal_type!(
    PitchBuffer,
    "Semitone pitch — 0.0 = C4, each 1.0 = one semitone. \
     Typical range: −60.0..+60.0."
);

signal_type!(
    ModBuffer,
    "Modulation-rate signal — sub-audio LFOs, envelopes, slow automation. \
     Bipolar −1.0..+1.0."
);

#[cfg(test)]
mod tests {
    use super::*;

    // ── AudioBuffer ───────────────────────────────────────────────────────────

    #[test]
    fn audio_buffer_planar_layout() {
        // Channels are stored contiguously (planar), not interleaved.
        // All ch0 samples come first, then all ch1 samples.
        let mut buf = AudioBuffer::new(2, 4);
        buf.channel_mut(0).copy_from_slice(&[1.0, 2.0, 3.0, 4.0]);
        buf.channel_mut(1).copy_from_slice(&[5.0, 6.0, 7.0, 8.0]);

        assert_eq!(buf.channel(0), &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(buf.channel(1), &[5.0, 6.0, 7.0, 8.0]);
    }

    #[test]
    fn audio_buffer_write_interleaved_matches_planar_source() {
        let mut buf = AudioBuffer::new(2, 3);
        buf.channel_mut(0).copy_from_slice(&[1.0, 2.0, 3.0]);
        buf.channel_mut(1).copy_from_slice(&[4.0, 5.0, 6.0]);

        let mut out = [0.0f32; 6];
        buf.write_interleaved(&mut out);

        // Interleaved: L R L R L R
        assert_eq!(out, [1.0, 4.0, 2.0, 5.0, 3.0, 6.0]);
    }

    #[test]
    fn audio_buffer_clear_zeros_all_channels() {
        let mut buf = AudioBuffer::new(2, 4);
        buf.channel_mut(0).fill(1.0);
        buf.channel_mut(1).fill(1.0);
        buf.clear();

        assert!(buf.channel(0).iter().all(|&s| s == 0.0));
        assert!(buf.channel(1).iter().all(|&s| s == 0.0));
    }

    #[test]
    fn audio_buffer_dimensions() {
        let buf = AudioBuffer::new(3, 128);
        assert_eq!(buf.channels(), 3);
        assert_eq!(buf.frames(), 128);
    }

    #[test]
    #[should_panic]
    fn audio_buffer_channel_out_of_bounds_panics() {
        let buf = AudioBuffer::new(2, 64);
        let _ = buf.channel(2);
    }

    // ── Signal buffer newtypes ────────────────────────────────────────────────

    #[test]
    fn cv_buffer_deref_gives_sample_slice() {
        let mut buf = CvBuffer::new(4);
        buf[0] = 0.5;
        buf[1] = -0.5;
        assert_eq!(buf[0], 0.5);
        assert_eq!(buf[1], -0.5);
        assert_eq!(buf.len(), 4);
    }

    #[test]
    fn phase_buffer_as_signal_frames_match_len() {
        let buf = PhaseBuffer::new(512);
        assert_eq!(buf.frames(), 512);
        assert_eq!(AsSignal::as_slice(&buf).len(), 512);
    }

    #[test]
    fn logic_buffer_clear_zeros_samples() {
        let mut buf = LogicBuffer::new(8);
        buf.as_slice_mut().fill(1.0);
        buf.clear();
        assert!(buf.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn pitch_buffer_deref_mut_allows_writes() {
        let mut buf = PitchBuffer::new(4);
        for (i, s) in buf.iter_mut().enumerate() {
            *s = i as f32;
        }
        assert_eq!(&buf[..], &[0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn mod_buffer_as_signal_mut_round_trips() {
        let mut buf = ModBuffer::new(2);
        buf.as_slice_mut()[0] = 0.25;
        assert_eq!(buf.as_slice()[0], 0.25);
    }
}
