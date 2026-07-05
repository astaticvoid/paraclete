use crate::surface::SurfaceEvent;
use crate::transport::TransportEvent;

/// A fixed-size owned UMP packet backed by `[u32; 4]` (the maximum UMP packet
/// size). `Copy` — fits in a single cache line.
///
/// This is the concrete type used in `Event::Midi2`. The generic buffer
/// parameter of `midi2::UmpMessage<B>` is fixed to `[u32; 4]` so that events
/// remain `Copy` and allocate no heap storage on the audio thread.
pub type UmpMessage = midi2::UmpMessage<[u32; 4]>;

// ── Sequencer parameter lock ──────────────────────────────────────────────────

/// Per-step parameter override emitted by the sequencer.
#[derive(Clone, Copy, Debug)]
pub struct ParamLockEvent {
    /// Target node ID.
    pub node_id: u32,
    /// Hash of the parameter name. Stable per node type.
    /// Locks targeting unknown IDs are held in suspension until the
    /// parameter reappears in the node's CapabilityDocument.
    pub param_id: u32,
    /// Value to apply for this step's duration.
    pub value: f64,
}

// ── ExtendedEventSlab / ExtendedEventRef ─────────────────────────────────────

/// A reference into the cycle's `ExtendedEventSlab`.
/// Stub at P0 — infrastructure ships at Phase 3.
#[derive(Clone, Copy, Debug)]
pub struct ExtendedEventRef {
    /// Negotiated at connection time. Local to the connection.
    pub space_id: u16,
    pub event_type: u16,
    pub slab_offset: u32,
    pub size: u32,
}

/// Pre-allocated byte slab for rich custom events.
/// Present in `ProcessInput` but always empty at P0.
pub struct ExtendedEventSlab {
    data: Box<[u8]>,
}

impl ExtendedEventSlab {
    pub fn empty() -> Self {
        Self { data: Box::new([]) }
    }

    /// Returns `None` if `space_id` does not match `known_space_id`.
    /// Use this to safely skip extended events from other protocols.
    pub fn read_if_known(
        &self,
        event_ref: &ExtendedEventRef,
        known_space_id: u16,
    ) -> Option<&[u8]> {
        if event_ref.space_id != known_space_id {
            return None;
        }
        let start = event_ref.slab_offset as usize;
        let end = start + event_ref.size as usize;
        self.data.get(start..end)
    }
}

// ── Core event enum ───────────────────────────────────────────────────────────

/// Typed node event.
/// `Copy` — events live in pre-allocated slices; nodes copy them as needed.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Event {
    /// MIDI 2.0 Universal MIDI Packet (see ADR-013).
    /// Covers all performance events: note on/off, per-note expression,
    /// pitch bend, pressure, controllers, program change.
    /// Decompose via `midi2::channel_voice2::ChannelVoice2` match arms.
    Midi2(UmpMessage),

    /// Native hardware control event from a physical or emulated controller.
    /// Carries typed, controller-specific data (grid position, velocity, etc.)
    /// that cannot be cleanly expressed in UMP. See `SurfaceMappingNode` for
    /// the standard Surface → Midi2 translation layer.
    Surface(SurfaceEvent),

    /// Sequencer parameter lock — overrides a parameter for one step.
    ParamLock(ParamLockEvent),

    /// Clock domain state change (loop point, stop, start, reposition).
    Transport(TransportEvent),

    /// Tempo change in BPM.
    Tempo(f64),

    /// Custom extended event. Points into the cycle's ExtendedEventSlab.
    /// space_id is negotiated at connection time via ConnectionAgreement.
    /// Stub at P0 — infrastructure ships at Phase 3.
    /// Intentionally last: the compiler treats later variants as less likely
    /// when the match is exhaustive and earlier arms are common.
    Extended(ExtendedEventRef),
}

/// A timestamped event within the current processing buffer.
/// `sample_offset` is the sample index at which the event occurs.
#[derive(Clone, Copy, Debug)]
pub struct TimedEvent {
    /// Sample index within the current block (0 .. block_size − 1).
    pub sample_offset: u32,
    pub event: Event,
}

impl TimedEvent {
    pub fn new(sample_offset: u32, event: Event) -> Self {
        Self { sample_offset, event }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── UmpMessage ────────────────────────────────────────────────────────────

    fn make_note_on() -> UmpMessage {
        use midi2::channel_voice2::{ChannelVoice2, NoteOn};
        midi2::UmpMessage::from(ChannelVoice2::from(NoteOn::<[u32; 4]>::new()))
    }

    #[test]
    fn ump_message_is_copy() {
        let a = make_note_on();
        let b = a;
        let _ = a;
        let _ = b;
    }

    #[test]
    fn ump_message_channel_voice2_roundtrips_through_event() {
        let ump = make_note_on();
        let event = Event::Midi2(ump);
        assert!(matches!(event, Event::Midi2(_)));
    }

    // ── Event enum ────────────────────────────────────────────────────────────

    /// Regression guard for ADR-013.
    #[test]
    fn event_enum_has_midi2_not_decomposed_note_variants() {
        let event = Event::Midi2(make_note_on());

        let kind = match event {
            Event::Midi2(_)      => "midi2",
            Event::Surface(_)   => "surface",
            Event::ParamLock(_)  => "param_lock",
            Event::Transport(_)  => "transport",
            Event::Tempo(_)      => "tempo",
            Event::Extended(_)   => "extended",
        };
        assert_eq!(kind, "midi2");
    }

    #[test]
    fn event_is_copy_and_clone() {
        let original = Event::Tempo(120.0);
        let copied = original;
        let cloned = original.clone();
        assert!(matches!(copied, Event::Tempo(_)));
        assert!(matches!(cloned, Event::Tempo(_)));
    }

    #[test]
    fn timed_event_carries_sample_offset_and_event() {
        let evt = TimedEvent::new(63, Event::Tempo(140.0));
        assert_eq!(evt.sample_offset, 63);
        assert!(matches!(evt.event, Event::Tempo(_)));
    }

    #[test]
    fn timed_event_is_copy() {
        let a = TimedEvent::new(0, Event::Tempo(120.0));
        let b = a;
        assert_eq!(a.sample_offset, b.sample_offset);
    }

    // ── ExtendedEventSlab ────────────────────────────────────────────────────────

    #[test]
    fn extended_event_slab_empty_returns_none() {
        let slab = ExtendedEventSlab::empty();
        let s = ExtendedEventRef { space_id: 1, event_type: 0, slab_offset: 0, size: 4 };
        assert!(slab.read_if_known(&s, 1).is_none());
    }

    #[test]
    fn extended_event_slab_wrong_space_id_returns_none() {
        let slab = ExtendedEventSlab::empty();
        let s = ExtendedEventRef { space_id: 42, event_type: 0, slab_offset: 0, size: 0 };
        assert!(slab.read_if_known(&s, 99).is_none());
    }
}
