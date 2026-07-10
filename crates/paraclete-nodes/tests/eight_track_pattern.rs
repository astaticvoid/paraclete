/// Integration test: 8-track pattern fires all expected steps.
///
/// Drives each Sequencer directly with synthetic TransportEvents.
/// Step index is encoded as the MIDI note number (step N → note N), so fired
/// steps are identified by note value rather than by tick timing — the test is
/// independent of the sequencer's internal tick-period behaviour.
use std::collections::HashSet;

use paraclete_node_api::{
    AudioBuffer, Event, EventOutputBuffer, ExtendedEventSlab, Node,
    ProcessInput, ProcessOutput, TransportEvent, TransportFlags,
    TransportInfo, UmpMessage, TICKS_PER_BEAT,
    midi::ChannelVoice2,
};
use paraclete_nodes::{Sequencer, TRACKS};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn mk_transport(tick: u32, playing: bool, global_start: bool) -> paraclete_node_api::TimedEvent {
    paraclete_node_api::TimedEvent::new(0, Event::Transport(TransportEvent {
        domain_id: 0,
        bar: 1, beat: 0, tick,
        ticks_per_beat: TICKS_PER_BEAT,
        bpm: 140.0,
        time_sig_num: 4, time_sig_den: 4,
        flags: TransportFlags { playing, global_start, ..TransportFlags::default() },
    }))
}

fn run_seq(seq: &mut Sequencer, events: &[paraclete_node_api::TimedEvent]) -> Vec<Event> {
    let block = 64usize;
    let mut audio = AudioBuffer::new(2, block);
    let mut events_out = EventOutputBuffer::new(256);
    let transport = TransportInfo::default();
    let slab = ExtendedEventSlab::empty();
    let audio_ptr: *mut AudioBuffer = &mut audio;
    let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
    let mut outs = [audio_ref];
    let input = ProcessInput {
        audio_inputs: &[], signal_inputs: &[], events,
        transport: &transport, sample_rate: 44100.0, block_size: block,
        extended_events: &slab, commands: &[],
    };
    let mut output = ProcessOutput {
        audio_outputs: &mut outs, signal_outputs: &mut [],
        events_out: &mut events_out,
    };
    seq.process(&input, &mut output);
    events_out.as_slice().iter().map(|e| e.event).collect()
}

fn note_on_notes(events: &[Event]) -> Vec<u8> {
    events.iter().filter_map(|e| {
        if let Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(msg))) = e {
            Some(u8::from(msg.note_number()))
        } else {
            None
        }
    }).collect()
}

/// Load a sequencer with step N → note N so fired steps are identified by note value.
/// Run for 5000 ticks (well beyond two full 16-step cycles at any tempo), collect
/// all NoteOn note values, then deduplicate — returns the set of step indices fired.
fn fired_steps(seq: &mut Sequencer, preset_steps: &[usize]) -> HashSet<usize> {
    // Activate ParameterBank before use.
    seq.activate(44100.0, 64);

    // Set each step's note = its index. Inactive steps get note=index but active=false.
    for i in 0u8..16 {
        let active = preset_steps.contains(&(i as usize));
        seq.set_step(i as usize, i, 40_000, active);
    }

    let mut all_notes: Vec<u8> = Vec::new();

    // global_start
    let start_events = run_seq(seq, &[mk_transport(0, true, true)]);
    all_notes.extend(note_on_notes(&start_events));

    // Run 5000 ticks — covers > 2 full 16-step patterns at any step duration ≤ 300 ticks.
    for tick in 1u32..5000 {
        let events = run_seq(seq, &[mk_transport(tick, true, false)]);
        all_notes.extend(note_on_notes(&events));
    }

    // Note value == step index. Deduplicate so multiple-pattern firings don't matter.
    all_notes.iter().map(|&n| n as usize).collect()
}

// ── Core test ─────────────────────────────────────────────────────────────────

/// Each of the 8 tracks fires NoteOn events at exactly its declared step positions.
#[test]
fn eight_track_pattern_fires_all_sounds_at_correct_steps() {
    for preset in TRACKS {
        let mut seq = Sequencer::with_name(preset.name);
        seq.set_node_id(1);

        let got      = fired_steps(&mut seq, preset.steps);
        let expected: HashSet<usize> = preset.steps.iter().copied().collect();

        assert_eq!(
            got, expected,
            "Track '{}': steps fired = {:?}, expected = {:?}",
            preset.name, sorted(&got), sorted(&expected),
        );
    }
}

fn sorted(set: &HashSet<usize>) -> Vec<usize> {
    let mut v: Vec<usize> = set.iter().copied().collect();
    v.sort_unstable();
    v
}

// ── Secondary tests ───────────────────────────────────────────────────────────

/// All 8 tracks have at least one active step — no silent tracks.
#[test]
fn all_eight_tracks_have_active_steps() {
    assert_eq!(TRACKS.len(), 8);
    for preset in TRACKS {
        assert!(!preset.steps.is_empty(),
            "track '{}' has no active steps", preset.name);
    }
}

/// Step counts match the design intent (documents expected density per track).
#[test]
fn pattern_step_counts_match_design() {
    let expected: &[(&str, usize)] = &[
        ("Kick",   4),  // 4-on-the-floor
        ("Snare",  2),  // backbeat 2/4
        ("Hat CH", 8),  // eighth notes
        ("Hat OH", 4),  // off-beat eighths
        ("Perc A", 2),
        ("Perc B", 2),
        ("FX",     2),
        ("Bass",   4),  // mirrors kick
    ];
    assert_eq!(TRACKS.len(), expected.len());
    for (preset, &(name, count)) in TRACKS.iter().zip(expected) {
        assert_eq!(preset.name, name, "unexpected track order");
        assert_eq!(
            preset.steps.len(), count,
            "track '{}' has {} active steps, expected {}",
            preset.name, preset.steps.len(), count,
        );
    }
}

/// All step indices are in range 0–15 with no duplicates per track.
#[test]
fn pattern_step_indices_are_valid() {
    for preset in TRACKS {
        for &step in preset.steps {
            assert!(step < 16,
                "track '{}' has out-of-range step {}", preset.name, step);
        }
        let unique: HashSet<usize> = preset.steps.iter().copied().collect();
        assert_eq!(unique.len(), preset.steps.len(),
            "track '{}' has duplicate step indices", preset.name);
    }
}

/// Kick and Bass share step positions — locked groove foundation.
#[test]
fn kick_and_bass_share_downbeat_positions() {
    let kick = TRACKS.iter().find(|t| t.name == "Kick").unwrap();
    let bass = TRACKS.iter().find(|t| t.name == "Bass").unwrap();
    let kick_set: HashSet<usize> = kick.steps.iter().copied().collect();
    let bass_set: HashSet<usize> = bass.steps.iter().copied().collect();
    assert_eq!(kick_set, bass_set,
        "Kick and Bass should share step positions");
}

/// No two adjacent tracks share the exact same step pattern (ensures variety).
#[test]
fn adjacent_tracks_have_distinct_patterns() {
    for pair in TRACKS.windows(2) {
        let a: HashSet<usize> = pair[0].steps.iter().copied().collect();
        let b: HashSet<usize> = pair[1].steps.iter().copied().collect();
        assert_ne!(a, b,
            "tracks '{}' and '{}' have identical patterns",
            pair[0].name, pair[1].name);
    }
}
