use crate::buffer::{
    AudioBuffer, CvBuffer, LogicBuffer, ModBuffer, PhaseBuffer, PitchBuffer,
};
use crate::event::{ExtendedEventSlab, TimedEvent};
use crate::transport::TransportInfo;

// ── Signal slot types ─────────────────────────────────────────────────────────
// Signal port connections (Cv, Phase, Logic, Pitch, Mod) land at P1 alongside
// the first CvSignal connections. These stub types carry the API shape; the
// full implementation (with proper buffer wiring) arrives in P1.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SignalPortKind {
    Cv,
    Phase,
    Logic,
    Pitch,
    Modulation,
}

/// Input signal slot. Created by the executor at connection time.
/// At P0, signal_inputs on ProcessInput is always an empty slice.
pub struct SignalInputSlot {
    pub port_id: u32,
    pub kind: SignalPortKind,
    // SAFETY: valid for the duration of the enclosing process() call.
    pub(crate) ptr: *const f32,
    pub(crate) frames: usize,
}

// SAFETY: The executor creates SignalInputSlots on the audio thread.
// No cross-thread sharing occurs during process().
unsafe impl Send for SignalInputSlot {}

/// Output signal slot. Created by the executor at connection time.
pub struct SignalOutputSlot {
    pub port_id: u32,
    pub kind: SignalPortKind,
    // SAFETY: valid for the duration of the enclosing process() call.
    pub(crate) ptr: *mut f32,
    pub(crate) frames: usize,
}

unsafe impl Send for SignalOutputSlot {}

// ── ProcessInput ──────────────────────────────────────────────────────────────

/// Everything a node reads during `process()`. Immutable.
///
/// All fields are `pub` — the executor (in `paraclete-runtime`) constructs
/// this struct directly. Node code should treat it as read-only.
pub struct ProcessInput<'a> {
    /// Audio input buffers, one per connected audio input port, in declaration order.
    pub audio_inputs: &'a [&'a AudioBuffer],

    /// Signal input slots. Always empty at P0.
    pub signal_inputs: &'a [SignalInputSlot],

    /// Events for this node, pre-filtered and sorted by `sample_offset`.
    pub events: &'a [TimedEvent],

    /// Clock state at the start of this buffer.
    pub transport: &'a TransportInfo,

    pub sample_rate: f32,
    pub block_size: usize,

    /// ExtendedEventSlab (extended event slab). Stub at P0.
    pub extended_events: &'a ExtendedEventSlab,
}

impl<'a> ProcessInput<'a> {
    fn find_input(&self, port_id: u32, kind: SignalPortKind) -> &[f32] {
        for slot in self.signal_inputs {
            if slot.port_id == port_id {
                assert_eq!(slot.kind, kind, "signal port {port_id} type mismatch");
                // SAFETY: ptr is valid for the duration of this process() call.
                return unsafe { std::slice::from_raw_parts(slot.ptr, slot.frames) };
            }
        }
        panic!("no signal input with port_id {port_id}");
    }

    // The signal accessor methods return typed buffer references.
    // At P0 these always panic because signal_inputs is always empty.
    // P1 implementation will coerce the raw slice to the typed buffer via a
    // safe wrapper once the buffer layout is stabilised.

    pub fn cv(&self, port_id: u32) -> &CvBuffer {
        let _ = self.find_input(port_id, SignalPortKind::Cv);
        unimplemented!("signal port views land at P1")
    }

    pub fn phase(&self, port_id: u32) -> &PhaseBuffer {
        let _ = self.find_input(port_id, SignalPortKind::Phase);
        unimplemented!("signal port views land at P1")
    }

    pub fn logic(&self, port_id: u32) -> &LogicBuffer {
        let _ = self.find_input(port_id, SignalPortKind::Logic);
        unimplemented!("signal port views land at P1")
    }

    pub fn pitch(&self, port_id: u32) -> &PitchBuffer {
        let _ = self.find_input(port_id, SignalPortKind::Pitch);
        unimplemented!("signal port views land at P1")
    }

    pub fn modulation(&self, port_id: u32) -> &ModBuffer {
        let _ = self.find_input(port_id, SignalPortKind::Modulation);
        unimplemented!("signal port views land at P1")
    }
}

// ── ProcessOutput ─────────────────────────────────────────────────────────────

/// Everything a node writes during `process()`.
///
/// All fields are `pub` — the executor constructs this struct directly.
pub struct ProcessOutput<'a> {
    /// Audio output buffers, one per connected audio output port, in declaration order.
    /// Index 0 is the primary stereo output; additional outputs follow in port order.
    pub audio_outputs: &'a mut [&'a mut AudioBuffer],

    /// Signal output slots. Always empty at P0.
    pub signal_outputs: &'a mut [SignalOutputSlot],

    /// Outgoing events. The executor routes these to downstream nodes.
    pub events_out: &'a mut EventOutputBuffer,
}

impl<'a> ProcessOutput<'a> {
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

    pub fn cv_mut(&mut self, port_id: u32) -> &mut CvBuffer {
        let _ = self.find_output(port_id, SignalPortKind::Cv);
        unimplemented!("signal port views land at P1")
    }

    pub fn phase_mut(&mut self, port_id: u32) -> &mut PhaseBuffer {
        let _ = self.find_output(port_id, SignalPortKind::Phase);
        unimplemented!("signal port views land at P1")
    }

    pub fn logic_mut(&mut self, port_id: u32) -> &mut LogicBuffer {
        let _ = self.find_output(port_id, SignalPortKind::Logic);
        unimplemented!("signal port views land at P1")
    }

    pub fn pitch_mut(&mut self, port_id: u32) -> &mut PitchBuffer {
        let _ = self.find_output(port_id, SignalPortKind::Pitch);
        unimplemented!("signal port views land at P1")
    }

    pub fn modulation_mut(&mut self, port_id: u32) -> &mut ModBuffer {
        let _ = self.find_output(port_id, SignalPortKind::Modulation);
        unimplemented!("signal port views land at P1")
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

    /// Push an outgoing event. Panics if capacity exceeded.
    pub fn push(&mut self, event: TimedEvent) {
        assert!(
            self.events.len() < self.capacity,
            "EventOutputBuffer capacity ({}) exceeded",
            self.capacity
        );
        self.events.push(event);
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }

    pub fn as_slice(&self) -> &[TimedEvent] {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::AudioBuffer;
    use crate::event::{ExtendedEventSlab, TimedEvent};
    use crate::transport::TransportInfo;

    // ── EventOutputBuffer ─────────────────────────────────────────────────────

    #[test]
    fn event_output_buffer_starts_empty() {
        let buf = EventOutputBuffer::new(16);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn event_output_buffer_push_and_len() {
        use crate::event::Event;
        let mut buf = EventOutputBuffer::new(4);
        buf.push(TimedEvent::new(0, Event::Tempo(120.0)));
        buf.push(TimedEvent::new(1, Event::Tempo(130.0)));
        assert_eq!(buf.len(), 2);
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
        buf.push(TimedEvent::new(1, Event::Tempo(130.0))); // should panic
    }

    // ── ProcessInput / ProcessOutput structural split ─────────────────────────

    #[test]
    fn process_input_audio_inputs_are_immutable_references() {
        // ProcessInput provides &[&AudioBuffer] — read-only by construction.
        // ProcessOutput provides &mut [&mut AudioBuffer] — writable.
        // This test demonstrates the type-system split is correctly constructed.
        let buf = AudioBuffer::new(2, 64);
        let inputs: &[&AudioBuffer] = &[&buf];
        let transport = TransportInfo::default();
        let extended = ExtendedEventSlab::empty();

        let input = ProcessInput {
            audio_inputs: inputs,
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: 64,
            extended_events: &extended,
        };

        assert_eq!(input.audio_inputs.len(), 1);
        assert_eq!(input.audio_inputs[0].channels(), 2);
        assert_eq!(input.audio_inputs[0].frames(), 64);
        assert_eq!(input.block_size, 64);
        assert_eq!(input.sample_rate, 44100.0);
    }

    #[test]
    fn process_output_audio_outputs_are_mutable() {
        let mut buf = AudioBuffer::new(2, 64);
        let mut events_out = EventOutputBuffer::new(16);

        // Construct output — uses same unsafe lifetime bridge as the executor.
        let out_ptr: *mut AudioBuffer = &mut buf as *mut AudioBuffer;
        let out_ref: &mut AudioBuffer = unsafe { &mut *out_ptr };
        let mut outs: [&mut AudioBuffer; 1] = [out_ref];

        let output = ProcessOutput {
            audio_outputs: &mut outs,
            signal_outputs: &mut [],
            events_out: &mut events_out,
        };

        // Write through output — this is what a node does in process().
        output.audio_outputs[0].channel_mut(0).fill(0.5);
        assert_eq!(buf.channel(0)[0], 0.5);
    }
}
