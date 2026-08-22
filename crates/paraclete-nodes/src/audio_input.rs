// SPDX-License-Identifier: GPL-3.0-or-later
//! `AudioInputNode` — AN1's live-audio signal source (ADR-040 decision 1).
//!
//! An ordinary mono signal source in the graph: "monitoring is graph
//! wiring (input → mix), not a special path." Constructed with the
//! consumer half of a cross-thread `rtrb` ring fed by
//! `paraclete_hal::audio::InputStreamHandle::start()`'s producer —
//! **not** registry/YAML-constructed (`design/phases/an1-anamnesis.md`
//! D4): a zero-argument `NodeRegistry` constructor can't carry a live
//! consumer that only exists once real input hardware is open, so this
//! node is wired directly in `paraclete-app`'s `main.rs`, the same way
//! Launchpad/Digitakt/Keystep already are.

use paraclete_node_api::{Node, PortDescriptor, PortDirection, PortName, PortType, ProcessInput, ProcessOutput};

/// Drains a cross-thread ring of raw interleaved input samples each
/// cycle, downmixing to mono (D2 — `SampleData`/`Sampler` are mono-only
/// throughout, and downmixing at the node boundary keeps `paraclete-hal`
/// a thin transport layer).
pub struct AudioInputNode {
    consumer: rtrb::Consumer<f32>,
    /// The input device's native channel count (from
    /// `InputStreamHandle::start`'s return) — how many ring samples make
    /// up one downmixed output frame.
    channels: usize,
    ports: [PortDescriptor; 1],
}

impl AudioInputNode {
    pub const PORT_AUDIO_OUT: u32 = 0;

    pub fn new(consumer: rtrb::Consumer<f32>, channels: usize) -> Self {
        Self {
            consumer,
            channels: channels.max(1),
            ports: [PortDescriptor {
                id: Self::PORT_AUDIO_OUT,
                name: PortName::Static("audio_out"),
                direction: PortDirection::Output,
                port_type: PortType::Audio,
            }],
        }
    }
}

impl Node for AudioInputNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    /// Drains exactly `block_size` mono frames from the input ring each
    /// cycle (averaging `channels` interleaved ring samples per output
    /// frame). Underrun — the ring running dry mid-block, meaning the
    /// input callback thread fell behind — silence-pads the remainder of
    /// the block rather than writing a torn/partial-averaged frame; this
    /// mirrors the tolerant posture `OutputRing::drain`'s underrun guard
    /// already takes on the symmetric output side
    /// (`paraclete-hal/src/audio.rs`).
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        let Some(out) = output.audio_outputs.first_mut() else {
            return;
        };
        let frames = input.block_size.min(out.frames());
        let channels = self.channels;
        let dst = out.channel_mut(0);

        let mut filled = 0usize;
        'frames: for frame in dst.iter_mut().take(frames) {
            let mut sum = 0.0f32;
            for _ in 0..channels {
                match self.consumer.pop() {
                    Ok(s) => sum += s,
                    Err(_) => break 'frames,
                }
            }
            *frame = sum / channels as f32;
            filled += 1;
        }
        for frame in &mut dst[filled..frames] {
            *frame = 0.0;
        }
        for frame in &mut dst[frames..] {
            *frame = 0.0;
        }
    }

    fn type_name(&self) -> &'static str {
        "AudioInputNode"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{AudioBuffer, EventOutputBuffer, ExtendedEventSlab, TransportInfo};

    /// Drives `node.process()` once against a fresh `block`-frame buffer
    /// and returns the rendered mono channel, mirroring the harness shape
    /// `oscillator.rs`'s tests already use (`run_osc`).
    fn run(node: &mut AudioInputNode, block: usize) -> Vec<f32> {
        run_prefilled(node, block, 0.0)
    }

    /// Same as `run`, but the destination channel is pre-filled with
    /// `sentinel` before `process()` runs. `AudioBuffer::new` zero-inits,
    /// and `NodeExecutor` also clears every node's output before calling
    /// `process()` in production (`executor.rs:655`) — so `run`'s plain
    /// zero-initialized buffer can't actually distinguish "process() wrote
    /// silence" from "process() wrote nothing and the buffer was already
    /// silent." A non-zero sentinel makes the silence-padding claim (D2,
    /// §2.1's underrun posture) something the underrun tests genuinely
    /// prove, not just something they'd pass either way (review finding).
    fn run_prefilled(node: &mut AudioInputNode, block: usize, sentinel: f32) -> Vec<f32> {
        let mut audio = AudioBuffer::new(2, block);
        for ch in 0..audio.channels() {
            audio.channel_mut(ch).fill(sentinel);
        }
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        let audio_ptr: *mut AudioBuffer = &mut audio as *mut AudioBuffer;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];

        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: block,
            extended_events: &slab,
            commands: &[],
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
        node.process(&input, &mut output);
        audio.channel(0)[..block].to_vec()
    }

    #[test]
    fn mono_input_passes_through_unchanged() {
        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(64);
        for i in 0..8 {
            producer.push(i as f32 * 0.1).unwrap();
        }
        let mut node = AudioInputNode::new(consumer, 1);
        let out = run(&mut node, 8);
        for (i, &s) in out.iter().enumerate() {
            assert!((s - i as f32 * 0.1).abs() < 1e-6);
        }
    }

    #[test]
    fn stereo_input_downmixes_to_mono_by_averaging() {
        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(64);
        // Two interleaved stereo frames: (1.0, -1.0) and (0.5, 0.5).
        for &s in &[1.0, -1.0, 0.5, 0.5] {
            producer.push(s).unwrap();
        }
        let mut node = AudioInputNode::new(consumer, 2);
        let out = run(&mut node, 2);
        assert!((out[0] - 0.0).abs() < 1e-6, "1.0 and -1.0 average to 0.0");
        assert!((out[1] - 0.5).abs() < 1e-6, "0.5 and 0.5 average to 0.5");
    }

    #[test]
    fn underrun_silence_pads_the_remainder_of_the_block() {
        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(64);
        // Only 3 frames' worth available; a 8-frame block is requested.
        for i in 0..3 {
            producer.push(i as f32 + 1.0).unwrap();
        }
        let mut node = AudioInputNode::new(consumer, 1);
        // Sentinel-prefilled (not zero) — proves the shortfall is actively
        // overwritten with silence, not left however the buffer arrived.
        let out = run_prefilled(&mut node, 8, 999.0);
        assert_eq!(&out[..3], &[1.0, 2.0, 3.0]);
        assert!(out[3..].iter().all(|&s| s == 0.0), "shortfall must silence-pad, not carry stale data");
    }

    #[test]
    fn empty_ring_produces_a_fully_silent_block_without_panicking() {
        let (_producer, consumer) = rtrb::RingBuffer::<f32>::new(64);
        let mut node = AudioInputNode::new(consumer, 1);
        let out = run_prefilled(&mut node, 16, 999.0);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn a_torn_frame_at_the_underrun_boundary_is_silenced_not_partially_averaged() {
        // Stereo (channels=2), but only ONE sample of the final frame's
        // pair is available — the partial frame must not leak through as
        // a half-computed average; it must read as silence like every
        // other frame past the shortfall.
        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(64);
        producer.push(1.0).unwrap();
        producer.push(1.0).unwrap(); // frame 0: complete stereo pair
        producer.push(0.5).unwrap(); // frame 1: only the left channel arrived
        let mut node = AudioInputNode::new(consumer, 2);
        let out = run_prefilled(&mut node, 2, 999.0);
        assert!((out[0] - 1.0).abs() < 1e-6);
        assert_eq!(out[1], 0.0, "the torn frame must be silence, not 0.5 or the sentinel");
    }
}
