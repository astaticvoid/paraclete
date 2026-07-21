use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::action::Action;
use crate::model::{Dir, Mode};

static TRACK_KEYS: &[KeyCode] = &[
    KeyCode::Char('q'),
    KeyCode::Char('w'),
    KeyCode::Char('e'),
    KeyCode::Char('r'),
    KeyCode::Char('u'),
    KeyCode::Char('i'),
    KeyCode::Char('o'),
    KeyCode::Char('p'),
];

static STEP_KEYS: &[KeyCode] = &[
    KeyCode::Char('a'),
    KeyCode::Char('s'),
    KeyCode::Char('d'),
    KeyCode::Char('f'),
    KeyCode::Char('j'),
    KeyCode::Char('k'),
    KeyCode::Char('l'),
    KeyCode::Char(';'),
];

pub fn map_key(mode: Mode, ev: &KeyEvent) -> Action {
    match mode {
        Mode::Seq => map_global(ev).unwrap_or_else(|| map_seq(ev)),
        Mode::Perf => map_global(ev).unwrap_or(Action::Noop),
    }
}

fn map_global(ev: &KeyEvent) -> Option<Action> {
    match ev.code {
        KeyCode::Char('c') if ev.modifiers == KeyModifiers::CONTROL => Some(Action::Quit),
        KeyCode::Tab => {
            let dir = if ev.modifiers.contains(KeyModifiers::SHIFT) {
                Dir::Prev
            } else {
                Dir::Next
            };
            Some(Action::CycleMode(dir))
        }
        KeyCode::Char(' ') => Some(Action::PlayToggle),
        KeyCode::Esc => Some(Action::Noop),
        _ => {
            track_idx(ev.code).map(Action::SelectTrack)
        }
    }
}

fn map_seq(ev: &KeyEvent) -> Action {
    match ev.code {
        KeyCode::Char('[') => Action::PageWindow(Dir::Prev),
        KeyCode::Char(']') => Action::PageWindow(Dir::Next),
        _ => step_col(ev.code).map(|col| Action::ToggleStep { col }).unwrap_or(Action::Noop),
    }
}

fn track_idx(code: KeyCode) -> Option<usize> {
    TRACK_KEYS.iter().position(|k| *k == code)
}

fn step_col(code: KeyCode) -> Option<usize> {
    STEP_KEYS.iter().position(|k| *k == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn global_keys_in_both_modes() {
        for mode in [Mode::Seq, Mode::Perf] {
            assert!(matches!(map_key(mode, &ctrl_key('c')), Action::Quit));
            assert!(matches!(map_key(mode, &key(' ')), Action::PlayToggle));
            assert!(matches!(
                map_key(mode, &key('q')),
                Action::SelectTrack(0)
            ));
            assert!(matches!(
                map_key(mode, &key('p')),
                Action::SelectTrack(7)
            ));
        }
    }

    #[test]
    fn seq_home_row_toggles_steps() {
        assert!(matches!(
            map_key(Mode::Seq, &key('a')),
            Action::ToggleStep { col: 0 }
        ));
        assert!(matches!(
            map_key(Mode::Seq, &key(';')),
            Action::ToggleStep { col: 7 }
        ));
        assert!(matches!(
            map_key(Mode::Seq, &key('s')),
            Action::ToggleStep { col: 1 }
        ));
    }

    #[test]
    fn seq_page_window_keys() {
        assert!(matches!(map_key(Mode::Seq, &key('[')), Action::PageWindow(Dir::Prev)));
        assert!(matches!(map_key(Mode::Seq, &key(']')), Action::PageWindow(Dir::Next)));
    }

    #[test]
    fn unknown_key_is_noop() {
        assert!(matches!(map_key(Mode::Seq, &key('z')), Action::Noop));
    }

    #[test]
    fn tab_cycles_modes() {
        assert!(matches!(
            map_key(Mode::Seq, &KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Action::CycleMode(Dir::Next)
        ));
        assert!(matches!(
            map_key(Mode::Seq, &KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT)),
            Action::CycleMode(Dir::Prev)
        ));
    }
}
