// SPDX-License-Identifier: GPL-3.0-or-later
//! DAW transport translation — CLAP → Paraclete TransportInfo/TransportEvent.
//! See ADR-024.

use paraclete_node_api::{TransportEvent, TransportFlags, TransportInfo, TICKS_PER_BEAT};

/// Subset of CLAP transport flags used by the adapter.
/// Values match the CLAP spec (clap/include/clap/events.h).
/// Bits 0-3 are the HAS_* validity flags; state flags start at bit 4.
pub const CLAP_TRANSPORT_HAS_BEATS_TIMELINE: u32 = 1 << 1;
pub const CLAP_TRANSPORT_IS_PLAYING: u32 = 1 << 4;
pub const CLAP_TRANSPORT_IS_RECORDING: u32 = 1 << 5;
pub const CLAP_TRANSPORT_IS_LOOP_ACTIVE: u32 = 1 << 6;

/// CLAP beattime uses fixed-point: beat value = raw / CLAP_BEATTIME_FACTOR.
const CLAP_BEATTIME_FACTOR: i64 = 1 << 31;

/// Translate DAW transport state into Paraclete types.
///
/// `flags`          — CLAP transport flags bitfield
/// `tempo`          — host BPM (from clap_event_transport.tempo)
/// `song_pos_beats` — song position in fixed-point beats (CLAP_BEATTIME_FACTOR per beat);
///                    only read when CLAP_TRANSPORT_HAS_BEATS_TIMELINE is set
/// `prev_playing`   — whether the transport was playing in the previous process() call
///
/// Returns `(TransportInfo, Option<TransportEvent>)`. The event is `Some` only on
/// a state transition: `global_start` when `!prev_playing && playing`, `global_stop`
/// when `prev_playing && !playing`. Returns `None` if the playing state is unchanged.
///
/// The caller extracts these scalar fields from the raw C struct so that this function
/// stays testable in pure Rust without constructing FFI structs.
pub fn translate_transport(
    flags: u32,
    tempo: f64,
    song_pos_beats: i64,
    prev_playing: bool,
) -> (TransportInfo, Option<TransportEvent>) {
    let playing = (flags & CLAP_TRANSPORT_IS_PLAYING) != 0;
    let recording = (flags & CLAP_TRANSPORT_IS_RECORDING) != 0;
    let looping = (flags & CLAP_TRANSPORT_IS_LOOP_ACTIVE) != 0;
    let has_beats = (flags & CLAP_TRANSPORT_HAS_BEATS_TIMELINE) != 0;

    // Convert fixed-point beat position → bar/beat/tick (assumes 4/4 for bar calc).
    // song_pos_beats is only valid when HAS_BEATS_TIMELINE is set.
    let (bar, beat, tick) = if has_beats {
        let beat_f64 = song_pos_beats as f64 / CLAP_BEATTIME_FACTOR as f64;
        let total_beats = beat_f64.max(0.0).floor() as u64; // clamp: pre-roll positions map to 0
        let tick_frac = (beat_f64 - total_beats as f64).max(0.0);
        (
            (total_beats / 4) as i32 + 1, // 1-based, 4/4 assumed
            (total_beats % 4) as u32,
            (tick_frac * TICKS_PER_BEAT as f64) as u32,
        )
    } else {
        (1, 0, 0)
    };

    let info = TransportInfo {
        domain_id: 0,
        bar,
        beat,
        tick,
        ticks_per_beat: TICKS_PER_BEAT,
        bpm: tempo,
        time_sig_num: 4,
        time_sig_den: 4,
        playing,
        recording,
        looping,
    };

    // Only emit a TransportEvent on state transitions to avoid resetting nodes every cycle.
    let event = if !prev_playing && playing {
        Some(TransportEvent {
            domain_id: 0,
            bar,
            beat,
            tick,
            ticks_per_beat: TICKS_PER_BEAT,
            bpm: tempo,
            time_sig_num: 4,
            time_sig_den: 4,
            flags: TransportFlags {
                playing,
                recording,
                looping,
                global_start: true,
                ..TransportFlags::default()
            },
        })
    } else if prev_playing && !playing {
        Some(TransportEvent {
            domain_id: 0,
            bar,
            beat,
            tick,
            ticks_per_beat: TICKS_PER_BEAT,
            bpm: tempo,
            time_sig_num: 4,
            time_sig_den: 4,
            flags: TransportFlags {
                global_stop: true,
                ..TransportFlags::default()
            },
        })
    } else {
        None
    };

    (info, event)
}
