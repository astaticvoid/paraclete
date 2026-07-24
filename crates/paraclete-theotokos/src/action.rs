use crate::model::{Dir, Mag, Screen, Slot};
use paraclete_node_api::NodeCommand;

pub const CMD_CLOCK_START: u32 = 16;
pub const CMD_CLOCK_STOP: u32 = 17;
pub const CMD_TOGGLE_STEP: u32 = 16;
pub const GRID_STEPS: usize = 16;
/// TK1 C5: lock command family (mirrors Sequencer constants).
pub const CMD_SET_LOCK_TARGET: u32 = 33;
pub const CMD_SET_STEP_LOCK: u32 = 34;
pub const CMD_CLEAR_STEP_LOCK: u32 = 35;

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Quit,
    CycleMode(Dir),
    PlayToggle,
    SelectTrack(usize),
    ToggleStep { col: usize },
    PageWindow(Dir),
    SelectParamPage(usize),
    Jog { slot: Slot, dir: Dir, mag: Mag },
    ToggleMute(usize),
    FocusStep,
    ReleaseFocus,
    ClearAllLocks,
    ClearSlotLocks,
    Colon,
    PatternSelect(u8),
    Yank,
    Paste,
    Leader,
    ToggleHelp,
    Noop,

    // ── TK2 C2 (additive — §0 A4): new panel-grammar actions. Nothing
    // produces these yet outside `input::button_to_action`'s pure tests;
    // lib.rs wiring lands in C3.
    /// D6/D11: TRK-hold or PTN-hold + trig, pattern arm.
    SelectPattern(usize),
    /// D5/D12: a trig fired with grid-rec off (`CMD_TRIG_NOW`, TK2 C1).
    LiveTrig { col: usize },
    /// D8: FUNC+top/bottom-row trig, resolved against the active page's
    /// encoder bank.
    EncoderJog { col: usize, dir: Dir, mag: Mag },
    /// D12: the REC button toggles `grid_rec` (grid-programming vs. live).
    ToggleGridRec,
    /// D12: KIT/SETTINGS/SAMPLING/TEMPO/SONG/MUTE navigate to a `Screen`.
    OpenScreen(Screen),
    /// D12/OQ-T23: YES-tap on the Tempo screen.
    TapTempo,
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
            Action::CycleMode(_)
            | Action::SelectTrack(_)
            | Action::PageWindow(_)
            | Action::SelectParamPage(_)
            | Action::Jog { .. }
            | Action::FocusStep
            | Action::ReleaseFocus
            | Action::ClearAllLocks
            | Action::ClearSlotLocks
            | Action::Colon
            | Action::PatternSelect(_)
            | Action::Yank
            | Action::Paste
            | Action::Leader
            | Action::ToggleHelp
            // TK2 C2: not yet wired to an engine command (C3+ territory);
            // these resolve as state-only until lib.rs consumes them.
            | Action::SelectPattern(_)
            | Action::LiveTrig { .. }
            | Action::EncoderJog { .. }
            | Action::ToggleGridRec
            | Action::OpenScreen(_)
            | Action::TapTempo => Outcome::StateOnly,
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
