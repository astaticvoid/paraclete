use crate::buffer::{
    AudioBuffer, CvBuffer, PhaseBuffer, PitchBuffer,
};
use crate::command::NodeCommand;
use crate::debug::{DebugEvent, DebugEventKind};
use crate::event::{ExtendedEventSlab, TimedEvent};
use crate::transport::TransportInfo;

// ── Signal slot types ─────────────────────────────────────────────────────────

/// Distinguishes between the five signal port types used in Paraclete's signal graph.
/// Signal ports carry per-sample f32 data between nodes; each kind has distinct semantics.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SignalPortKind {
    /// Control voltage — generic 0–1 (or ±1) modulation signal.
    Cv,
    /// Phase accumulator — 0.0–1.0 ramp used to drive oscillators.
    Phase,
    /// Gate or trigger — 1.0 = active, 0.0 = inactive per sample.
    Logic,
    /// MIDI pitch (0–127 range, can be fractional for pitch-bend).
    Pitch,
    /// LFO or envelope modulation — typically ±1.0 or 0.0–1.0.
    Modulation,
}

/// Input signal slot. Created by the executor at connection time.
pub struct SignalInputSlot {
    pub port_id: u32,
    pub kind: SignalPortKind,
    // SAFETY: valid for the duration of the enclosing process() call.
    pub(crate) ptr: *const f32,
    pub(crate) frames: usize,
}

// SAFETY: The executor creates SignalInputSlots on the audio thread.
unsafe impl Send for SignalInputSlot {}

impl SignalInputSlot {
    pub fn new(port_id: u32, kind: SignalPortKind, data: &[f32]) -> Self {
        Self { port_id, kind, ptr: data.as_ptr(), frames: data.len() }
    }
}

/// Output signal slot. Created by the executor at connection time.
pub struct SignalOutputSlot {
    pub port_id: u32,
    pub kind: SignalPortKind,
    // SAFETY: valid for the duration of the enclosing process() call.
    pub(crate) ptr: *mut f32,
    pub(crate) frames: usize,
}

unsafe impl Send for SignalOutputSlot {}

impl SignalOutputSlot {
    pub fn new(port_id: u32, kind: SignalPortKind, data: &mut [f32]) -> Self {
        Self { port_id, kind, ptr: data.as_mut_ptr(), frames: data.len() }
    }
}

/// Module-level silence buffer shared by `logic()` and `modulation()` fallback paths.
/// Large enough for any expected block size; capacity is asserted at call time.
static SILENCE: [f32; 65536] = [0.0; 65536];

// ── ProcessInput ──────────────────────────────────────────────────────────────

/// Everything a node reads during `process()`. Immutable.
pub struct ProcessInput<'a> {
    pub audio_inputs: &'a [&'a AudioBuffer],
    pub signal_inputs: &'a [SignalInputSlot],
    pub events: &'a [TimedEvent],
    pub transport: &'a TransportInfo,
    pub sample_rate: f32,
    pub block_size: usize,
    pub extended_events: &'a ExtendedEventSlab,
    /// NodeCommands addressed to this node for this cycle.
    pub commands: &'a [NodeCommand],
}

impl<'a> ProcessInput<'a> {
    /// CV input for a connected port.
    ///
    /// **Not yet implemented (P5):** signal port buffer wiring is deferred. This
    /// method always returns an empty buffer regardless of graph connections. Nodes
    /// that declare Cv ports will compile and link against this API, but will receive
    /// silence until the executor wires signal buffers at `build_executor()` time.
    pub fn cv(&self, _port_id: u32) -> &CvBuffer {
        static SILENT: std::sync::OnceLock<CvBuffer> = std::sync::OnceLock::new();
        SILENT.get_or_init(|| CvBuffer::new(0))
    }

    /// Phase input for a connected port. **Not yet implemented (P5) — always returns silence.**
    pub fn phase(&self, _port_id: u32) -> &PhaseBuffer {
        static SILENT: std::sync::OnceLock<PhaseBuffer> = std::sync::OnceLock::new();
        SILENT.get_or_init(|| PhaseBuffer::new(0))
    }

    /// Logic (gate/trigger) input. Returns a block_size slice of 1.0/0.0 values.
    /// Returns zeros if the port is unconnected (no `SignalInputSlot` with this id).
    pub fn logic(&self, port_id: u32) -> &[f32] {
        for slot in self.signal_inputs {
            if slot.port_id == port_id && slot.kind == SignalPortKind::Logic {
                // SAFETY: ptr is valid for the duration of this process() call.
                return unsafe { std::slice::from_raw_parts(slot.ptr, slot.frames) };
            }
        }
        assert!(
            self.block_size <= SILENCE.len(),
            "block_size {} exceeds SILENCE buffer capacity {}",
            self.block_size,
            SILENCE.len(),
        );
        &SILENCE[..self.block_size]
    }

    /// Pitch input for a connected port. **Not yet wired in executor — always returns silence.**
    pub fn pitch(&self, _port_id: u32) -> &PitchBuffer {
        static SILENT: std::sync::OnceLock<PitchBuffer> = std::sync::OnceLock::new();
        SILENT.get_or_init(|| PitchBuffer::new(0))
    }

    /// Modulation input. Returns a block_size slice of f32 values.
    /// Returns zeros if the port is unconnected (no `SignalInputSlot` with this id).
    pub fn modulation(&self, port_id: u32) -> &[f32] {
        for slot in self.signal_inputs {
            if slot.port_id == port_id && slot.kind == SignalPortKind::Modulation {
                // SAFETY: ptr is valid for the duration of this process() call.
                return unsafe { std::slice::from_raw_parts(slot.ptr, slot.frames) };
            }
        }
        assert!(
            self.block_size <= SILENCE.len(),
            "block_size {} exceeds SILENCE buffer capacity {}",
            self.block_size,
            SILENCE.len(),
        );
        &SILENCE[..self.block_size]
    }

    /// CV signal input (audio-rate CV). Returns a block_size slice of f32 values.
    /// Returns zeros if the port is unconnected (no `SignalInputSlot` with this id).
    /// Used by `LoopBreakNode` and other nodes that declare `PortType::Cv` ports.
    pub fn cv_signal(&self, port_id: u32) -> &[f32] {
        for slot in self.signal_inputs {
            if slot.port_id == port_id && slot.kind == SignalPortKind::Cv {
                // SAFETY: ptr is valid for the duration of this process() call.
                return unsafe { std::slice::from_raw_parts(slot.ptr, slot.frames) };
            }
        }
        assert!(
            self.block_size <= SILENCE.len(),
            "block_size {} exceeds SILENCE buffer capacity {}",
            self.block_size,
            SILENCE.len(),
        );
        &SILENCE[..self.block_size]
    }
}

// ── ProcessOutput ─────────────────────────────────────────────────────────────

/// Everything a node writes during `process()`. Mutable.
///
/// Provides access to audio output buffers, signal output slots, and the
/// outgoing event queue. The runtime pre-allocates all buffers; nodes write
/// into them without allocating.
pub struct ProcessOutput<'a> {
    pub audio_outputs: &'a mut [&'a mut AudioBuffer],
    pub signal_outputs: &'a mut [SignalOutputSlot],
    pub events_out: &'a mut EventOutputBuffer,
    /// Pre-allocated debug event buffer (executor-owned, cleared each cycle).
    /// `None` when debug logging is disabled (zero-cost in shipping builds).
    pub debug: Option<&'a mut Vec<DebugEvent>>,
}

impl<'a> ProcessOutput<'a> {
    /// Create a ProcessOutput with debug logging disabled (the default).
    /// Use the struct literal directly only when passing a debug buffer
    /// (executor only — see ADR-035 Part B).
    pub fn new(
        audio_outputs: &'a mut [&'a mut AudioBuffer],
        signal_outputs: &'a mut [SignalOutputSlot],
        events_out: &'a mut EventOutputBuffer,
    ) -> Self {
        Self { audio_outputs, signal_outputs, events_out, debug: None }
    }

    fn find_output(&mut self, port_id: u32, kind: SignalPortKind) -> &mut [f32] {
        for slot in self.signal_outputs.iter_mut() {
            if slot.port_id == port_id {
                assert_eq!(slot.kind, kind, "signal port {port_id} type mismatch");
                // SAFETY: ptr is valid for the duration of this process() call.
                return unsafe { std::slice::from_raw_parts_mut(slot.ptr, slot.frames) };
            }
        }
        panic!("no signal output with port_id {port_id}");
    }

    /// Write a Modulation port output. Returns a mutable slice of block_size values.
    /// Panics if port_id is not a registered Modulation output port.
    pub fn mod_output_mut(&mut self, port_id: u32) -> &mut [f32] {
        self.find_output(port_id, SignalPortKind::Modulation)
    }

    /// Write a Logic port output. Returns a mutable slice of block_size values.
    /// Panics if port_id is not a registered Logic output port.
    pub fn logic_output_mut(&mut self, port_id: u32) -> &mut [f32] {
        self.find_output(port_id, SignalPortKind::Logic)
    }

    /// Write a CV signal port output. Returns a mutable slice of block_size values.
    /// Panics if port_id is not a registered Cv output port.
    /// Used by `LoopBreakNode` and other nodes that declare `PortType::Cv` output ports.
    pub fn cv_signal_output_mut(&mut self, port_id: u32) -> &mut [f32] {
        self.find_output(port_id, SignalPortKind::Cv)
    }

    /// Emit a debug event from the current node.
    ///
    /// `node_id` is filled by the executor after `process()` returns.
    /// `sample_offset` is relative to the start of the current audio block.
    /// When `debug` is `None` (logging disabled), this is a no-op.
    pub fn emit_debug(&mut self, sample_offset: u32, kind: DebugEventKind, arg0: i64, arg1: f64) {
        if let Some(buf) = &mut self.debug {
            buf.push(DebugEvent {
                sample_offset,
                node_id: 0, // executor fills in
                kind,
                arg0,
                arg1,
            });
        }
    }
}

// ── EventOutputBuffer ─────────────────────────────────────────────────────────

/// Pre-allocated buffer for events emitted by a node during processing.
pub struct EventOutputBuffer {
    events: Vec<TimedEvent>,
    capacity: usize,
}

impl EventOutputBuffer {
    pub fn new(capacity: usize) -> Self {
        Self { events: Vec::with_capacity(capacity), capacity }
    }

    pub fn push(&mut self, event: TimedEvent) {
        assert!(
            self.events.len() < self.capacity,
            "EventOutputBuffer capacity ({}) exceeded",
            self.capacity
        );
        self.events.push(event);
    }

    pub fn len(&self) -> usize { self.events.len() }
    pub fn is_empty(&self) -> bool { self.events.is_empty() }
    pub fn clear(&mut self) { self.events.clear(); }
    pub fn as_slice(&self) -> &[TimedEvent] { &self.events }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TimedEvent;

    #[test]
    fn event_output_buffer_starts_empty() {
        let buf = EventOutputBuffer::new(16);
        assert!(buf.is_empty());
    }

    #[test]
    fn event_output_buffer_push_and_len() {
        use crate::event::Event;
        let mut buf = EventOutputBuffer::new(4);
        buf.push(TimedEvent::new(0, Event::Tempo(120.0)));
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn event_output_buffer_clear_resets_to_empty() {
        use crate::event::Event;
        let mut buf = EventOutputBuffer::new(4);
        buf.push(TimedEvent::new(0, Event::Tempo(120.0)));
        buf.clear();
        assert!(buf.is_empty());
    }

    #[test]
    #[should_panic(expected = "capacity")]
    fn event_output_buffer_exceeding_capacity_panics() {
        use crate::event::Event;
        let mut buf = EventOutputBuffer::new(1);
        buf.push(TimedEvent::new(0, Event::Tempo(120.0)));
        buf.push(TimedEvent::new(1, Event::Tempo(130.0)));
    }

    #[test]
    fn process_input_has_commands_field() {
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let cmd = NodeCommand { target_id: 1, type_id: 0, arg0: 0, arg1: 0.5 };
        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: 64,
            extended_events: &slab,
            commands: &[cmd],
        };
        assert_eq!(input.commands.len(), 1);
    }

    #[test]
    fn process_input_audio_inputs_are_immutable_references() {
        let buf = AudioBuffer::new(2, 64);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let input = ProcessInput {
            audio_inputs: &[&buf],
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: 64,
            extended_events: &slab,
            commands: &[],
        };
        assert_eq!(input.audio_inputs.len(), 1);
        assert_eq!(input.block_size, 64);
    }

    #[test]
    fn logic_input_unconnected_returns_zeros() {
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: 64,
            extended_events: &slab,
            commands: &[],
        };
        let gate = input.logic(0);
        assert_eq!(gate.len(), 64);
        assert!(gate.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn mod_output_mut_returns_block_size_slice() {
        let block = 64usize;
        let mut out_buf = vec![0.0f32; block];
        let mut events_out = EventOutputBuffer::new(16);
        let out_slot = SignalOutputSlot::new(0, SignalPortKind::Modulation, &mut out_buf);
        let mut sig_outs = [out_slot];
        let mut output = ProcessOutput::new(
            &mut [],
            &mut sig_outs,
            &mut events_out,
        );
        let slice = output.mod_output_mut(0);
        assert_eq!(slice.len(), block);
    }

    #[test]
    fn process_output_audio_outputs_are_mutable() {
        let mut buf = AudioBuffer::new(2, 64);
        let mut events_out = EventOutputBuffer::new(16);
        let out_ptr: *mut AudioBuffer = &mut buf as *mut AudioBuffer;
        let out_ref: &mut AudioBuffer = unsafe { &mut *out_ptr };
        let mut outs = [out_ref];
        let output = ProcessOutput::new(
            &mut outs,
            &mut [],
            &mut events_out,
        );
        output.audio_outputs[0].channel_mut(0).fill(0.5);
        assert_eq!(buf.channel(0)[0], 0.5);
    }
}
