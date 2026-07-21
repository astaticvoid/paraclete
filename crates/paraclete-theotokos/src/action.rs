use paraclete_node_api::NodeCommand;
use crate::model::Dir;

pub const CMD_CLOCK_START: u32 = 16;
pub const CMD_CLOCK_STOP: u32 = 17;
pub const CMD_TOGGLE_STEP: u32 = 16;
pub const PAGE_SIZE: usize = 8;

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Quit,
    CycleMode(Dir),
    PlayToggle,
    SelectTrack(usize),
    ToggleStep { col: usize },
    PageWindow(Dir),
    Noop,
}

#[derive(Debug)]
pub enum Outcome {
    Command(NodeCommand),
    StateOnly,
    Quit,
    Noop,
}

impl Action {
    pub fn execute(
        self,
        clock_id: u32,
        seq_id: u32,
        page_window: usize,
        playing: bool,
    ) -> Outcome {
        match self {
            Action::Quit => Outcome::Quit,
            Action::CycleMode(_) => Outcome::StateOnly,
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
            Action::SelectTrack(_) => Outcome::StateOnly,
            Action::ToggleStep { col } => {
                let step = (page_window * PAGE_SIZE + col) as i64;
                Outcome::Command(NodeCommand {
                    target_id: seq_id,
                    type_id: CMD_TOGGLE_STEP,
                    arg0: step,
                    arg1: 0.0,
                })
            }
            Action::PageWindow(_) => Outcome::StateOnly,
            Action::Noop => Outcome::Noop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_toggle_when_playing_sends_stop() {
        let out = Action::PlayToggle.execute(1, 0, 0, true);
        assert!(matches!(out, Outcome::Command(cmd) if cmd.target_id == 1 && cmd.type_id == CMD_CLOCK_STOP));
    }

    #[test]
    fn play_toggle_when_stopped_sends_start() {
        let out = Action::PlayToggle.execute(1, 0, 0, false);
        assert!(matches!(out, Outcome::Command(cmd) if cmd.target_id == 1 && cmd.type_id == CMD_CLOCK_START));
    }

    #[test]
    fn toggle_step_offset_includes_page_window() {
        let out = Action::ToggleStep { col: 5 }.execute(0, 10, 0, false);
        assert!(matches!(out, Outcome::Command(cmd) if cmd.target_id == 10 && cmd.arg0 == 5));

        let out = Action::ToggleStep { col: 3 }.execute(0, 10, 2, false);
        assert!(matches!(out, Outcome::Command(cmd) if cmd.arg0 == 19));
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
