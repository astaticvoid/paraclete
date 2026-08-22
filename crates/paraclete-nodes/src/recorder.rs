// SPDX-License-Identifier: GPL-3.0-or-later
//! `RecorderNode` — AN1's always-on ring recorder (ADR-040 decision 2).
//!
//! "A node with one signal input and no output obligation" (ADR-040,
//! verbatim) — it owns a pre-allocated circular buffer, overwrite-oldest,
//! written every `process()` cycle from whatever it's wired to hear
//! (external input, mix bus, a track tap — ADR-029 topology). Capture is
//! retroactive by construction: the ring keeps recording regardless of
//! whether anyone's watching; a later `CMD_RECORDER_CAPTURE` pulls the
//! last N frames out of what's already there.
//!
//! C2 ships the ring itself plus threshold-arm detection. The
//! capture-drain command (`CMD_RECORDER_CAPTURE`) lands in C3 alongside
//! the capture-transfer plane it depends on
//! (`design/phases/an1-anamnesis.md` D5-D7, D10, §2.2).

use paraclete_node_api::{
    Node, PortDescriptor, PortDirection, PortName, PortType, ProcessInput, ProcessOutput,
    StateBusValue,
};

/// New node-specific `NodeCommand::type_id`s. `command.rs` reserves 0-15
/// for universal commands; 16+ is per-node-type namespace — `RecorderNode`
/// owns this vocabulary independently of every other node's own 16+ ids
/// (e.g. the Sequencer's 16-44, CLAUDE.md) since dispatch is by node type,
/// not a shared global table.
pub const CMD_RECORDER_ARM_THRESHOLD: u32 = 16;
pub const CMD_RECORDER_DISARM: u32 = 17;

/// D7: fixed duration, not bars — a bar's frame length depends on live
/// tempo, which can change mid-session; the ring's byte budget is fixed
/// once at `activate()` and must never depend on a value that moves
/// afterward. 60s covers design.md's largest listed Retro option (8
/// bars) at any tempo >= 32 BPM.
const DEFAULT_CAPACITY_SECONDS: f32 = 60.0;

/// One-pole peak-follower time constant for threshold-arm detection.
/// Not user-tunable in AN1 — a conventional fast-attack default.
const ENVELOPE_TIME_CONSTANT_SECONDS: f32 = 0.01;

/// A silent-signal floor so `20.0 * x.log10()` never evaluates `log10(0.0)`
/// (`-inf`, which is a legal `f32`/`f64` but a needless edge case to carry
/// through a StateBus `Float` unnecessarily).
const MIN_LINEAR: f32 = 1e-6;

pub struct RecorderNode {
    capacity_seconds: f32,
    ring: Box<[f32]>,
    write_pos: usize,
    envelope: f32,
    envelope_coeff: f32,
    /// True while waiting for a threshold crossing
    /// (`CMD_RECORDER_ARM_THRESHOLD`); cleared on crossing (one-shot) or
    /// by `CMD_RECORDER_DISARM`.
    armed: bool,
    threshold_db: f64,
    /// Latches `true` on a threshold crossing while `armed`; cleared only
    /// by the next `CMD_RECORDER_ARM_THRESHOLD` or `CMD_RECORDER_DISARM`
    /// — not automatically on the next cycle. The app main loop polls the
    /// published form of this (`/node/{id}/state/armed`, see
    /// `published_state`) for a `false -> true` edge as its cue to issue
    /// a capture (§2.4); if this reset itself on the very next audio
    /// cycle, a main-loop tick landing between the trigger and the next
    /// `process()` call could miss the edge entirely.
    fired: bool,
    node_id: u32,
    ports: [PortDescriptor; 1],
    state_path_cache: std::sync::OnceLock<[String; 2]>,
}

impl RecorderNode {
    pub const PORT_AUDIO_IN: u32 = 0;

    pub fn new() -> Self {
        Self {
            capacity_seconds: DEFAULT_CAPACITY_SECONDS,
            ring: Box::new([]),
            write_pos: 0,
            envelope: 0.0,
            envelope_coeff: 1.0,
            armed: false,
            threshold_db: -12.0,
            fired: false,
            node_id: 0,
            ports: [PortDescriptor {
                id: Self::PORT_AUDIO_IN,
                name: PortName::Static("audio_in"),
                direction: PortDirection::Input,
                port_type: PortType::Audio,
            }],
            state_path_cache: std::sync::OnceLock::new(),
        }
    }

    /// Ring capacity in frames — `0` before `activate()` has run.
    pub fn capacity_frames(&self) -> usize {
        self.ring.len()
    }
}

impl Default for RecorderNode {
    fn default() -> Self {
        Self::new()
    }
}

impl Node for RecorderNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    /// The ring's one and only allocation (D7) — main thread, before any
    /// `process()` call, per the `Node` trait's own contract for
    /// sample-rate-dependent DSP state.
    fn activate(&mut self, sample_rate: f32, _block_size: usize) {
        let frames = (self.capacity_seconds * sample_rate).max(1.0) as usize;
        self.ring = vec![0.0; frames].into_boxed_slice();
        self.write_pos = 0;
        self.envelope_coeff =
            1.0 - (-1.0 / (ENVELOPE_TIME_CONSTANT_SECONDS * sample_rate)).exp();
    }

    fn set_node_id(&mut self, id: u32) {
        self.node_id = id;
    }

    /// Writes every incoming frame into the ring (silence when
    /// unconnected — the ring still advances in wall-clock time
    /// regardless of whether anything is wired, matching "recorders run
    /// always"), tracks a peak envelope, and detects a threshold crossing
    /// while armed. Never allocates, blocks, or locks — the ring is fixed
    /// by `activate()` and every other field is a plain scalar.
    fn process(&mut self, input: &ProcessInput, _output: &mut ProcessOutput) {
        if self.ring.is_empty() {
            // activate() hasn't run yet (shouldn't happen in the real
            // executor, which always activates before the first
            // process() call, but a safe no-op here costs nothing and
            // avoids a modulo-by-zero if it ever did).
            return;
        }
        let cap = self.ring.len();

        for cmd in input.commands {
            match cmd.type_id {
                CMD_RECORDER_ARM_THRESHOLD => {
                    self.threshold_db = cmd.arg1;
                    self.armed = true;
                    self.fired = false;
                }
                CMD_RECORDER_DISARM => {
                    self.armed = false;
                    self.fired = false;
                }
                _ => {}
            }
        }

        for f in 0..input.block_size {
            let mut sample = 0.0f32;
            for audio_in in input.audio_inputs {
                let chs = audio_in.channels();
                if chs == 0 {
                    continue;
                }
                let mut sum = 0.0f32;
                for ch in 0..chs {
                    sum += audio_in.channel(ch).get(f).copied().unwrap_or(0.0);
                }
                sample += sum / chs as f32;
            }

            self.ring[self.write_pos] = sample;
            self.write_pos = (self.write_pos + 1) % cap;

            let peak = sample.abs();
            self.envelope += (peak - self.envelope) * self.envelope_coeff;

            if self.armed && !self.fired {
                let envelope_db = 20.0 * self.envelope.max(MIN_LINEAR).log10();
                if envelope_db as f64 >= self.threshold_db {
                    self.armed = false;
                    self.fired = true;
                }
            }
        }
    }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        let id = self.node_id;
        let paths = self.state_path_cache.get_or_init(|| {
            [
                format!("/node/{id}/state/armed"),
                format!("/node/{id}/state/peak_db"),
            ]
        });
        // The published "armed" bit is the fired-edge the app watches for
        // (§2.4: a false -> true transition is the capture cue) — not the
        // internal "currently waiting for a crossing" flag, which the app
        // has no independent need to observe.
        buf.push((
            paths[0].clone(),
            StateBusValue::Float(if self.fired { 1.0 } else { 0.0 }),
        ));
        let peak_db = 20.0 * self.envelope.max(MIN_LINEAR).log10();
        buf.push((paths[1].clone(), StateBusValue::Float(peak_db as f64)));
    }

    fn type_name(&self) -> &'static str {
        "RecorderNode"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{AudioBuffer, EventOutputBuffer, ExtendedEventSlab, NodeCommand, TransportInfo};

    fn cmd(type_id: u32, arg0: i64, arg1: f64) -> NodeCommand {
        NodeCommand { target_id: 0, type_id, arg0, arg1 }
    }

    /// Drives one `process()` cycle with `samples` as a single-channel
    /// input (silence-padded/truncated to `block`), returning nothing —
    /// tests inspect `node`'s private fields directly (same module).
    fn run(node: &mut RecorderNode, block: usize, samples: &[f32], commands: &[NodeCommand]) {
        let mut audio_in = AudioBuffer::new(1, block);
        for (i, &s) in samples.iter().take(block).enumerate() {
            audio_in.channel_mut(0)[i] = s;
        }
        let mut audio_out = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        let audio_out_ptr: *mut AudioBuffer = &mut audio_out as *mut AudioBuffer;
        let audio_out_ref: &mut AudioBuffer = unsafe { &mut *audio_out_ptr };
        let mut outs = [audio_out_ref];

        let input = ProcessInput {
            audio_inputs: &[&audio_in],
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: block,
            extended_events: &slab,
            commands,
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
        node.process(&input, &mut output);
    }

    #[test]
    fn activate_allocates_ring_sized_to_capacity_seconds() {
        let mut node = RecorderNode::new();
        assert_eq!(node.capacity_frames(), 0, "no allocation before activate()");
        node.activate(1000.0, 64);
        assert_eq!(node.capacity_frames(), 60_000, "60s @ 1000Hz");
    }

    #[test]
    fn process_before_activate_is_a_safe_noop() {
        let mut node = RecorderNode::new();
        run(&mut node, 4, &[1.0, 1.0, 1.0, 1.0], &[]);
        assert_eq!(node.write_pos, 0);
    }

    #[test]
    fn process_writes_input_into_the_ring_and_advances_write_pos() {
        let mut node = RecorderNode::new();
        node.activate(1000.0, 64); // 60,000-frame ring
        run(&mut node, 4, &[0.1, 0.2, 0.3, 0.4], &[]);
        assert_eq!(&node.ring[..4], &[0.1, 0.2, 0.3, 0.4]);
        assert_eq!(node.write_pos, 4);
    }

    #[test]
    fn ring_wraps_around_overwriting_the_oldest_samples() {
        let mut node = RecorderNode::new();
        node.activate(10.0, 700); // 600-frame ring at 10 Hz
        assert_eq!(node.capacity_frames(), 600);
        let samples: Vec<f32> = (0..650).map(|i| i as f32).collect();
        run(&mut node, 650, &samples, &[]);
        // 650 samples into a 600-frame ring: indices 600..650 (values
        // 600.0..650.0) wrap and overwrite ring[0..50].
        assert_eq!(node.write_pos, 50);
        assert_eq!(&node.ring[..50], &samples[600..650]);
        assert_eq!(&node.ring[50..600], &samples[50..600]);
    }

    #[test]
    fn unconnected_recorder_still_advances_write_pos_with_silence() {
        let mut node = RecorderNode::new();
        node.activate(1000.0, 64);
        let mut audio_out = AudioBuffer::new(2, 8);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let audio_out_ptr: *mut AudioBuffer = &mut audio_out as *mut AudioBuffer;
        let audio_out_ref: &mut AudioBuffer = unsafe { &mut *audio_out_ptr };
        let mut outs = [audio_out_ref];
        let input = ProcessInput {
            audio_inputs: &[], // nothing wired
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: 8,
            extended_events: &slab,
            commands: &[],
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
        node.process(&input, &mut output);
        assert_eq!(node.write_pos, 8, "the ring must still advance in wall-clock time");
        assert!(node.ring[..8].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn threshold_arm_and_crossing_latches_fired_until_rearmed() {
        let mut node = RecorderNode::new();
        node.activate(1000.0, 64);
        run(&mut node, 1, &[0.0], &[cmd(CMD_RECORDER_ARM_THRESHOLD, 0, -6.0)]);
        assert!(node.armed && !node.fired);

        // A loud, sustained signal — well above -6dB after the envelope
        // has had a few cycles to rise.
        for _ in 0..50 {
            run(&mut node, 4, &[1.0, 1.0, 1.0, 1.0], &[]);
        }
        assert!(node.fired, "a sustained full-scale signal must cross -6dB");
        assert!(!node.armed, "crossing disarms — one-shot per arm");

        // fired stays latched across further cycles, even silent ones.
        run(&mut node, 4, &[0.0, 0.0, 0.0, 0.0], &[]);
        assert!(node.fired, "fired must not self-clear on the next cycle");
    }

    /// Arming while the envelope is already above the threshold (no prior
    /// "was below, is now above" edge) fires on the very next evaluated
    /// frame — there is no separate rising-edge gate distinct from the
    /// armed/threshold check itself. Review finding: this path was
    /// logically correct but untested; nothing in `design.md` demands
    /// edge-detection semantics, so this documents the actual (intended)
    /// behavior rather than leaving it unverified.
    #[test]
    fn arming_while_already_above_threshold_fires_immediately() {
        let mut node = RecorderNode::new();
        node.activate(1000.0, 64);
        // Get the envelope well above -6dB first, unarmed.
        for _ in 0..50 {
            run(&mut node, 4, &[1.0, 1.0, 1.0, 1.0], &[]);
        }
        assert!(!node.fired, "unarmed — a crossing must not fire on its own");
        // Arm now, while the signal is already loud.
        run(&mut node, 4, &[1.0, 1.0, 1.0, 1.0], &[cmd(CMD_RECORDER_ARM_THRESHOLD, 0, -6.0)]);
        assert!(node.fired, "arming into an already-elevated envelope fires immediately");
    }

    /// Review finding: every other test drives a single input source.
    /// `process()` is written to sum across `input.audio_inputs` (each
    /// downmixed to mono first) — matching `AudioOutputNode`'s parallel-
    /// send precedent (`lib.rs`) — but nothing proved that without this.
    #[test]
    fn multiple_simultaneous_audio_inputs_are_summed_not_just_the_first() {
        let mut node = RecorderNode::new();
        node.activate(1000.0, 64);
        let mut a = AudioBuffer::new(1, 4);
        let mut b = AudioBuffer::new(1, 4);
        a.channel_mut(0).fill(0.3);
        b.channel_mut(0).fill(0.2);

        let mut audio_out = AudioBuffer::new(2, 4);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let audio_out_ptr: *mut AudioBuffer = &mut audio_out as *mut AudioBuffer;
        let audio_out_ref: &mut AudioBuffer = unsafe { &mut *audio_out_ptr };
        let mut outs = [audio_out_ref];
        let input = ProcessInput {
            audio_inputs: &[&a, &b],
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: 4,
            extended_events: &slab,
            commands: &[],
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
        node.process(&input, &mut output);

        assert!(
            node.ring[..4].iter().all(|&s| (s - 0.5).abs() < 1e-6),
            "two parallel sends (0.3, 0.2) must sum to 0.5, not read only the first"
        );
    }

    #[test]
    fn disarm_clears_both_armed_and_fired() {
        let mut node = RecorderNode::new();
        node.activate(1000.0, 64);
        run(&mut node, 1, &[0.0], &[cmd(CMD_RECORDER_ARM_THRESHOLD, 0, -60.0)]);
        for _ in 0..10 {
            run(&mut node, 4, &[1.0, 1.0, 1.0, 1.0], &[]);
        }
        assert!(node.fired);
        run(&mut node, 1, &[0.0], &[cmd(CMD_RECORDER_DISARM, 0, 0.0)]);
        assert!(!node.armed);
        assert!(!node.fired);
    }

    #[test]
    fn published_state_reports_fired_as_armed_and_a_peak_db_reading() {
        let mut node = RecorderNode::new();
        node.set_node_id(210);
        node.activate(1000.0, 64);
        run(&mut node, 1, &[0.0], &[cmd(CMD_RECORDER_ARM_THRESHOLD, 0, -6.0)]);
        for _ in 0..50 {
            run(&mut node, 4, &[1.0, 1.0, 1.0, 1.0], &[]);
        }
        let mut buf = Vec::new();
        node.published_state(&mut buf);
        let armed = buf.iter().find(|(p, _)| p == "/node/210/state/armed").unwrap();
        assert_eq!(armed.1, StateBusValue::Float(1.0));
        let peak = buf.iter().find(|(p, _)| p == "/node/210/state/peak_db").unwrap();
        match peak.1 {
            StateBusValue::Float(db) => assert!(db > -6.0, "a full-scale signal must read above -6dB"),
            _ => panic!("expected Float"),
        }
    }
}
