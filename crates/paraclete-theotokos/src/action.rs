use crate::model::{Dir, Mag, Screen, Slot};
use paraclete_node_api::NodeCommand;

pub const CMD_CLOCK_START: u32 = 16;
pub const CMD_CLOCK_STOP: u32 = 17;
/// ADR-046 T1 (mirrors `InternalClock::CMD_CLOCK_REWIND`): set position to
/// the window start, independent of `playing`.
pub const CMD_CLOCK_REWIND: u32 = 18;
pub const CMD_TOGGLE_STEP: u32 = 16;
pub const GRID_STEPS: usize = 16;
/// TK1 C5: lock command family (mirrors Sequencer constants).
pub const CMD_SET_LOCK_TARGET: u32 = 33;
pub const CMD_SET_STEP_LOCK: u32 = 34;
pub const CMD_CLEAR_STEP_LOCK: u32 = 35;
/// P10 C4 (mirrors `Sequencer::CMD_SET_PATTERN`).
pub const CMD_SET_PATTERN: u32 = 27;
/// TK2 C1 (mirrors `Sequencer::CMD_TRIG_NOW`, D5).
pub const CMD_TRIG_NOW: u32 = 38;
/// P10 C4 (mirrors `Sequencer::PATTERN_BANK_SIZE`, D9 clamp).
pub const PATTERN_BANK_SIZE: usize = 8;
/// P10 C1 (mirrors `Sequencer::CMD_CLEAR`). §0 A8: clears steps only —
/// locks survive; FUNC+PLAY (TK2 C4) pairs this with `CMD_CLEAR_STEP_LOCK`
/// per step.
pub const CMD_CLEAR: u32 = 18;
/// P10 C4 (mirrors `Sequencer::CMD_CHAIN_PUSH`/`CMD_CHAIN_CLEAR`, TK2 C6
/// Chain screen).
pub const CMD_CHAIN_PUSH: u32 = 31;
pub const CMD_CHAIN_CLEAR: u32 = 32;
/// P11 C6e (mirrors `paraclete_node_api::command::CMD_SET_PATTERN_MUTE`):
/// set/toggle the active pattern's muted flag. arg0: 0 = off, 1 = on,
/// 2 = toggle.
pub const CMD_SET_PATTERN_MUTE: u32 = 41;
/// P11 C6e (mirrors `paraclete_node_api::command::CMD_PREPARE_PATTERN_MUTE`):
/// defer a pattern-mute change to the next pattern wrap. arg0: 0 = off,
/// 1 = on.
pub const CMD_PREPARE_PATTERN_MUTE: u32 = 43;
/// P11 C6 (OQ-T25, mirrors `paraclete_node_api::command::CMD_LIVE_ERASE`):
/// arm/disarm live erasing (hold NO while the transport plays). arg0:
/// 0 = off, 1 = on. Disarmed on transport stop.
pub const CMD_LIVE_ERASE: u32 = 44;

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Quit,
    PlayToggle,
    SelectTrack(usize),
    ToggleStep { col: usize },
    PageWindow(Dir),
    /// D8's encoder bank reuses this dispatch/ramp machinery; unreachable
    /// from any key since the TK2 C3 wiring flip retired the TK1 arrow-jog
    /// trigger (§2 removed-bindings list) — numpad slots A/B/C only.
    Jog { slot: Slot, dir: Dir, mag: Mag },
    ToggleMute(usize),
    ClearAllLocks,
    ClearSlotLocks,
    Colon,
    ToggleHelp,
    Noop,

    // ── TK2 C2/C3 (§0 A4): panel-grammar actions.
    /// D6/D11: TRK-hold or PTN-hold + trig, pattern arm.
    SelectPattern(usize),
    /// D5/D12: a trig fired with grid-rec off (`CMD_TRIG_NOW`, TK2 C1).
    LiveTrig { col: usize },
    /// D8: FUNC+top/bottom-row trig, resolved against the active page's
    /// encoder bank.
    EncoderJog { col: usize, dir: Dir, mag: Mag },
    /// TK2.1 C1 (D5): renamed from `ToggleGridRec` — bare REC press. Toggles
    /// `Off ↔ Grid` on the kitty path (or applies the transport-derived
    /// fallback rule where key releases are unavailable); from `Live`,
    /// always returns to `Off`.
    ToggleRec,
    /// TK2.1 C1 (D5): REC held + PLAY (kitty path only) — arms `Live` and
    /// starts the transport.
    EnterLiveRec,
    /// D12: KIT/SETTINGS/SAMPLING/TEMPO/SONG/MUTE navigate to a `Screen`.
    OpenScreen(Screen),
    /// D12/OQ-T23: YES-tap on the Tempo screen.
    TapTempo,

    // ── TK2 C4 (D7/A8): FUNC+transport chords ──
    /// FUNC+REC: copy the active track's active pattern lane.
    CopyLane,
    /// FUNC+PLAY: clear the active track's pattern (§0 A8 — CMD_CLEAR
    /// plus a CMD_CLEAR_STEP_LOCK per step; CMD_CLEAR alone leaves locks).
    ClearLane,
    /// FUNC+STOP: paste the copied lane.
    PasteLane,

    /// TK2 C5 (§0 A11): the same Pg key pressed again while already on
    /// that page cycles its sub-page (pages over 8 params split rather
    /// than truncating; §0 A1 hypothesis — session).
    NextSubPage,

    // ── TK2 C6: Tempo/Chain screens (D12) ──
    /// Tempo screen: UP/DOWN nudge bpm by the given signed delta (±1,
    /// FUNC+UP/DOWN = ±0.1).
    NudgeBpm(f64),
    /// Chain screen: YES pushes the cursor pattern onto the volatile chain.
    ChainPush,
    /// Chain screen: NO/Backspace clears the chain.
    ChainClear,
    /// Chain screen: LEFT/RIGHT move the pattern-bank cursor.
    MoveChainCursor(Dir),
    /// D12: KIT echoes `reserved (kit)` — no screen exists for it in TK2.
    Echo(&'static str),

    // ── TK2.1 C5 (D9/D15) ──
    /// C5a: bare ENC toggles `Model.enc`.
    ToggleEnc,
    /// C5b: the latched path — Lock armed + the next Grid-mode trig sets
    /// `(active_track, col)` as the lock target.
    SetLockTarget(usize),
    /// C5b: Lock armed + a trig OUTSIDE Grid mode — the arm is consumed but
    /// there is no step to target, so the refusal is surfaced rather than
    /// silently ignored (#171).
    LockTargetRefused(usize),
    /// C5b: re-pressing the trig that set the target, pressing Lock again
    /// while a target is set, or Esc — all clear it.
    ClearLockTarget,

    /// ADR-046 T5: bare STOP — halt in place, then rewind to the window
    /// start (`CMD_CLOCK_STOP` + `CMD_CLOCK_REWIND`, in that order). This
    /// is the reference box's "stop" grammar: PLAY alone only pauses.
    Stop,

    // ── P11 C6 (Theotokos surfaces) ──
    /// C6e: TRK+FUNC+trig — toggle `pattern_mute` on track N's sequencer
    /// (`CMD_SET_PATTERN_MUTE` arg0 = 2).
    TogglePatternMute(usize),
    /// C6e: TRK+FUNC+Ctrl+trig — defer a pattern-mute change to the next
    /// pattern wrap (`CMD_PREPARE_PATTERN_MUTE`, arg0 read from the
    /// current `/node/{id}/state/pattern_muted`).
    PreparePatternMute(usize),
    /// C6a: the KIT screen's encoder scroll — moves the cursor one slot
    /// and page-aligns the list window.
    KitListScroll(Dir),
    /// C6a: KIT screen LOAD (trig 13) — `AppOp::KitLoad(kit_cursor)`.
    KitLoad,
    /// C6a: KIT screen SAVE (trig 14) — `AppOp::KitSaveAs("Kit N")`.
    KitSaveAs,
    /// C6a: KIT screen COMMIT (trig 15) — `AppOp::KitCommit`.
    KitCommit,
    /// C6a: KIT screen RELD (trig 16) — `AppOp::KitReload`.
    KitReload,
    /// C6b: FUNC+KIT — `AppOp::SetPerformMode(!current)`.
    TogglePerformMode,
    /// C6c: FUNC+YES — `AppOp::TempSave`.
    TempSave,
    /// C6c: FUNC+NO — `AppOp::TempReload`.
    TempReload,
}

#[derive(Debug)]
pub enum Outcome {
    Command(NodeCommand),
    StateOnly,
    Quit,
    Noop,
}

impl Action {
    pub fn execute(self, clock_id: u32, seq_id: u32, page_window: usize, playing: bool) -> Outcome {
        match self {
            Action::Quit => Outcome::Quit,
            Action::SelectTrack(_)
            | Action::PageWindow(_)
            | Action::Jog { .. }
            | Action::ClearAllLocks
            | Action::ClearSlotLocks
            | Action::Colon
            | Action::ToggleHelp
            // TK2 C2/C3: these are dispatched directly in lib.rs's own
            // match, not through `execute()` — the arms below exist only
            // so this match stays exhaustive.
            | Action::SelectPattern(_)
            | Action::LiveTrig { .. }
            | Action::EncoderJog { .. }
            | Action::ToggleRec
            | Action::EnterLiveRec
            | Action::OpenScreen(_)
            | Action::TapTempo
            // TK2 C4: dispatched directly in lib.rs (bus/pattern-length
            // access needed for ClearLane's per-step lock clears).
            | Action::CopyLane
            | Action::ClearLane
            | Action::PasteLane
            | Action::NextSubPage
            | Action::NudgeBpm(_)
            | Action::ChainPush
            | Action::ChainClear
            // ADR-046 T5: dispatched directly in lib.rs — needs two
            // commands (STOP + REWIND), which this enum's single-command
            // `Outcome::Command` cannot represent.
            | Action::Stop
            | Action::MoveChainCursor(_)
            | Action::Echo(_)
            | Action::ToggleEnc
            | Action::SetLockTarget(_)
            | Action::LockTargetRefused(_)
            | Action::ClearLockTarget
            // P11 C6: all dispatched directly in lib.rs — they need the
            // state bus, the selected track, or AppOp push access.
            | Action::TogglePatternMute(_)
            | Action::PreparePatternMute(_)
            | Action::KitListScroll(_)
            | Action::KitLoad
            | Action::KitSaveAs
            | Action::KitCommit
            | Action::KitReload
            | Action::TogglePerformMode
            | Action::TempSave
            | Action::TempReload => Outcome::StateOnly,
            Action::PlayToggle => {
                if playing {
                    Outcome::Command(NodeCommand {
                        target_id: clock_id,
                        type_id: CMD_CLOCK_STOP,
                        arg0: 0,
                        arg1: 0.0,
                    })
                } else {
                    Outcome::Command(NodeCommand {
                        target_id: clock_id,
                        type_id: CMD_CLOCK_START,
                        arg0: 0,
                        arg1: 0.0,
                    })
                }
            }
            Action::ToggleStep { col } => {
                let step = (page_window * GRID_STEPS + col) as i64;
                Outcome::Command(NodeCommand {
                    target_id: seq_id,
                    type_id: CMD_TOGGLE_STEP,
                    arg0: step,
                    arg1: 0.0,
                })
            }
            Action::Noop => Outcome::Noop,
            Action::ToggleMute(_) => Outcome::StateOnly,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_toggle_when_playing_sends_stop() {
        let out = Action::PlayToggle.execute(1, 0, 0, true);
        assert!(
            matches!(out, Outcome::Command(cmd) if cmd.target_id == 1 && cmd.type_id == CMD_CLOCK_STOP)
        );
    }

    #[test]
    fn play_toggle_when_stopped_sends_start() {
        let out = Action::PlayToggle.execute(1, 0, 0, false);
        assert!(
            matches!(out, Outcome::Command(cmd) if cmd.target_id == 1 && cmd.type_id == CMD_CLOCK_START)
        );
    }

    #[test]
    fn toggle_step_offset_includes_page_window() {
        let out = Action::ToggleStep { col: 5 }.execute(0, 10, 0, false);
        assert!(matches!(out, Outcome::Command(cmd) if cmd.target_id == 10 && cmd.arg0 == 5));

        let out = Action::ToggleStep { col: 3 }.execute(0, 10, 2, false);
        assert!(matches!(out, Outcome::Command(cmd) if cmd.arg0 == 35));
    }

    #[test]
    fn quit_action_produces_quit_outcome() {
        let out = Action::Quit.execute(0, 0, 0, false);
        assert!(matches!(out, Outcome::Quit));
    }

    #[test]
    fn noop_produces_noop() {
        let out = Action::Noop.execute(0, 0, 0, false);
        assert!(matches!(out, Outcome::Noop));
    }
}
