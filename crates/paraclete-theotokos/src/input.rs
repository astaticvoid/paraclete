use std::collections::{HashMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::action::Action;
use crate::model::{Dir, Mag, Mode, Screen, Slot};

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
    KeyCode::Char('z'),
    KeyCode::Char('x'),
    KeyCode::Char('c'),
    KeyCode::Char('v'),
    KeyCode::Char('m'),
    KeyCode::Char(','),
    KeyCode::Char('.'),
    KeyCode::Char('/'),
];

pub fn map_key(mode: Mode, ev: &KeyEvent) -> Action {
    match mode {
        Mode::Seq => map_global(ev).unwrap_or_else(|| map_seq(ev)),
        Mode::Perf => map_global(ev).unwrap_or_else(|| map_perf(ev)),
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
        KeyCode::Esc => Some(Action::ReleaseFocus),
        KeyCode::Char(';') if ev.modifiers == KeyModifiers::SHIFT => Some(Action::Colon),
        KeyCode::Enter => Some(Action::FocusStep),
        KeyCode::Backspace => {
            if ev.modifiers.contains(KeyModifiers::SHIFT) {
                Some(Action::ClearSlotLocks)
            } else {
                Some(Action::ClearAllLocks)
            }
        }
        KeyCode::Char('y') if ev.modifiers == KeyModifiers::SHIFT => Some(Action::Paste),
        KeyCode::Char('y') => Some(Action::Yank),
        KeyCode::Char('\\') => Some(Action::Leader),
        KeyCode::Char('?') => Some(Action::ToggleHelp),
        _ => {
            if let Some(col) = step_col(ev.code) {
                return Some(Action::ToggleStep { col });
            }
            if ev.modifiers.contains(KeyModifiers::SHIFT) {
                if let Some(i) = track_idx(ev.code) {
                    return Some(Action::ToggleMute(i));
                }
            }
            track_idx(ev.code).map(Action::SelectTrack)
        }
    }
}

fn map_seq(ev: &KeyEvent) -> Action {
    match ev.code {
        KeyCode::Char('-') => Action::PageWindow(Dir::Prev),
        KeyCode::Char('=') => Action::PageWindow(Dir::Next),
        KeyCode::Char('1') => Action::PatternSelect(0),
        KeyCode::Char('2') => Action::PatternSelect(1),
        KeyCode::Char('3') => Action::PatternSelect(2),
        KeyCode::Char('4') => Action::PatternSelect(3),
        KeyCode::Char('5') => Action::PatternSelect(4),
        KeyCode::Char('6') => Action::PatternSelect(5),
        KeyCode::Char('7') => Action::PatternSelect(6),
        KeyCode::Char('8') => Action::PatternSelect(7),
        _ => Action::Noop,
    }
}

fn map_perf(ev: &KeyEvent) -> Action {
    let mag = if ev.modifiers.contains(KeyModifiers::SHIFT) {
        Mag::Fine
    } else {
        Mag::Normal
    };
    match ev.code {
        KeyCode::Char('1') => Action::SelectParamPage(0),
        KeyCode::Char('2') => Action::SelectParamPage(1),
        KeyCode::Char('3') => Action::SelectParamPage(2),
        KeyCode::Char('4') => Action::SelectParamPage(3),
        KeyCode::Char('5') => Action::SelectParamPage(4),
        KeyCode::Char('6') => Action::SelectParamPage(5),
        KeyCode::Up => Action::Jog {
            slot: Slot::A,
            dir: Dir::Next,
            mag,
        },
        KeyCode::Down => Action::Jog {
            slot: Slot::A,
            dir: Dir::Prev,
            mag,
        },
        KeyCode::Right => Action::Jog {
            slot: Slot::B,
            dir: Dir::Next,
            mag,
        },
        KeyCode::Left => Action::Jog {
            slot: Slot::B,
            dir: Dir::Prev,
            mag,
        },
        _ => Action::Noop,
    }
}

fn track_idx(code: KeyCode) -> Option<usize> {
    TRACK_KEYS.iter().position(|k| *k == code)
}

fn step_col(code: KeyCode) -> Option<usize> {
    STEP_KEYS.iter().position(|k| *k == code)
}

// ── TK2 C2: panel model (pure types + mapping) ───────────────────────────
//
// Additive only (§0 A4): coexists with the TK1 `map_key`/`map_seq`/
// `map_perf` pipeline above until C3 flips `lib.rs`'s wiring and the old
// pipeline is deleted. Nothing here is called by `lib.rs` yet.

/// The physical panel surface (§2): one variant per labeled button,
/// independent of which physical key currently produces it. Names match
/// the `:bind`/`:unbind` verb vocabulary (D11), case-insensitively.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelButton {
    Trig1,
    Trig2,
    Trig3,
    Trig4,
    Trig5,
    Trig6,
    Trig7,
    Trig8,
    Trig9,
    Trig10,
    Trig11,
    Trig12,
    Trig13,
    Trig14,
    Trig15,
    Trig16,
    Trk,
    Ptn,
    Rec,
    Play,
    Stop,
    Pg1,
    Pg2,
    Pg3,
    Pg4,
    Pg5,
    Pg6,
    Kit,
    Settings,
    Sampling,
    Tempo,
    Yes,
    No,
    Up,
    Down,
    Left,
    Right,
    PagePrev,
    PageNext,
    Song,
    Keybd,
    Mute,
}

/// `col` 0..16 → the matching `PanelButton::TrigN`.
fn trig_button(col: usize) -> Option<PanelButton> {
    use PanelButton::*;
    const TABLE: [PanelButton; 16] = [
        Trig1, Trig2, Trig3, Trig4, Trig5, Trig6, Trig7, Trig8, Trig9, Trig10, Trig11, Trig12,
        Trig13, Trig14, Trig15, Trig16,
    ];
    TABLE.get(col).copied()
}

/// The inverse of `trig_button`: `None` for any non-trig button.
fn trig_col(button: PanelButton) -> Option<usize> {
    use PanelButton::*;
    match button {
        Trig1 => Some(0),
        Trig2 => Some(1),
        Trig3 => Some(2),
        Trig4 => Some(3),
        Trig5 => Some(4),
        Trig6 => Some(5),
        Trig7 => Some(6),
        Trig8 => Some(7),
        Trig9 => Some(8),
        Trig10 => Some(9),
        Trig11 => Some(10),
        Trig12 => Some(11),
        Trig13 => Some(12),
        Trig14 => Some(13),
        Trig15 => Some(14),
        Trig16 => Some(15),
        _ => None,
    }
}

/// The continuous grid's top row (§2): `q w e r t y u i` → Trig1..8.
const TOP_TRIG_ROW: [char; 8] = ['q', 'w', 'e', 'r', 't', 'y', 'u', 'i'];
/// The continuous grid's bottom row (§2): `a s d f g h j k` → Trig9..16.
const BOTTOM_TRIG_ROW: [char; 8] = ['a', 's', 'd', 'f', 'g', 'h', 'j', 'k'];

/// A normalized key for the user keymap (D11) and the built-in §2 table:
/// `Char` letters are always lowercase — see `func_held` (§0 A1), which
/// carries the case-implied FUNC bit separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyBinding {
    pub code: KeyCode,
}

/// The user keymap (D11): flat, global, no per-screen bindings. Empty by
/// default — C2 introduces the type; C8 adds YAML load/save + the `:bind`
/// family of verbs.
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    pub bindings: HashMap<KeyBinding, PanelButton>,
}

/// §0 A1: crossterm never delivers `Shift+letter` as lowercase+SHIFT —
/// legacy input sends the uppercase char (+SHIFT still set); kitty's
/// alternate-keys mode sends the uppercase char with SHIFT *cleared*.
/// FUNC is therefore held whenever the modifier flag is set, OR (for
/// letters specifically) the character itself arrived uppercase.
pub fn func_held(ev: &KeyEvent) -> bool {
    if ev.modifiers.contains(KeyModifiers::SHIFT) {
        return true;
    }
    matches!(ev.code, KeyCode::Char(c) if c.is_ascii_uppercase())
}

/// Case-folds a key code to the form the §2 table and the user keymap are
/// keyed on (§0 A1): letters always lowercase, everything else unchanged.
fn normalize_code(code: KeyCode) -> KeyCode {
    match code {
        KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
        other => other,
    }
}

/// TK2 C2 (D2/§2/D11): resolve a physical key to a `PanelButton` — user
/// bindings first, then the built-in §2 table. Modifiers never change
/// *which* button a key identifies (only `button_to_action`'s resolved
/// `Action` depends on FUNC/Ctrl) — case-folded per §0 A1 so kitty and
/// legacy terminals agree.
pub fn key_to_button(keymap: &Keymap, ev: KeyEvent) -> Option<PanelButton> {
    let binding = KeyBinding {
        code: normalize_code(ev.code),
    };
    if let Some(&button) = keymap.bindings.get(&binding) {
        return Some(button);
    }
    built_in_button(binding.code)
}

fn built_in_button(code: KeyCode) -> Option<PanelButton> {
    use PanelButton::*;
    if let KeyCode::Char(c) = code {
        if let Some(i) = TOP_TRIG_ROW.iter().position(|&k| k == c) {
            return trig_button(i);
        }
        if let Some(i) = BOTTOM_TRIG_ROW.iter().position(|&k| k == c) {
            return trig_button(8 + i);
        }
    }
    match code {
        KeyCode::Tab => Some(Trk),
        KeyCode::Char('p') => Some(Ptn),
        KeyCode::Char('z') => Some(Rec),
        KeyCode::Char('x') => Some(Play),
        KeyCode::Char('c') => Some(Stop),
        // A12: `Space` is a PLAY alias only — resolved as a transport-only
        // no-op under FUNC by `button_to_action`, not here.
        KeyCode::Char(' ') => Some(Play),
        KeyCode::Char('1') => Some(Pg1),
        KeyCode::Char('2') => Some(Pg2),
        KeyCode::Char('3') => Some(Pg3),
        KeyCode::Char('4') => Some(Pg4),
        KeyCode::Char('5') => Some(Pg5),
        KeyCode::Char('6') => Some(Pg6),
        KeyCode::Char('7') => Some(Kit),
        KeyCode::Char('8') => Some(Settings),
        KeyCode::Char('9') => Some(Sampling),
        KeyCode::Char('0') => Some(Tempo),
        KeyCode::Enter => Some(Yes),
        KeyCode::Esc => Some(No),
        KeyCode::Up => Some(Up),
        KeyCode::Down => Some(Down),
        KeyCode::Left => Some(Left),
        KeyCode::Right => Some(Right),
        KeyCode::Char('-') => Some(PagePrev),
        KeyCode::Char('=') => Some(PageNext),
        KeyCode::Char('o') => Some(Song),
        KeyCode::Char('m') => Some(Mute),
        KeyCode::Char('v') => Some(Keybd),
        _ => None,
    }
}

/// D6: which hold-prefix is armed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    Trk,
    Ptn,
}

/// D6 hold-chord state, both branches: `kitty = true` selects the
/// real-hold path (press arms, physical release disarms — wired to
/// crossterm release events in C3); `kitty = false` (the common case,
/// probed via `supports_keyboard_enhancement()` at startup) selects the
/// one-shot sticky fallback this struct implements today, amended by §0
/// A9 (a repeated same-prefix press is a no-op, not a toggle — auto-repeat
/// streams indistinguishable synthetic presses without release events).
#[derive(Debug, Default)]
pub struct HeldState {
    pub kitty: bool,
    pub armed: Option<Hold>,
    /// Physically-pressed panel buttons, kitty mode only — tracked so a
    /// release event can tell which prefix to drop. Unused by the sticky
    /// fallback below; wired alongside kitty release handling in C3.
    pub pressed: HashSet<PanelButton>,
}

impl HeldState {
    pub fn new(kitty: bool) -> Self {
        Self {
            kitty,
            armed: None,
            pressed: HashSet::new(),
        }
    }

    /// D6 (sticky fallback) + §0 A9: process one button PRESS. Returns
    /// `true` if the press was consumed by prefix arm/disarm bookkeeping
    /// itself (the caller must not also resolve an `Action` for it, since
    /// `Trk`/`Ptn` on their own are not actions); `false` means the caller
    /// should resolve the button normally via `button_to_action` — reading
    /// `self.armed` as it stood *before* this call, since a completed
    /// chord disarms as a side effect of the same press.
    pub fn on_press(&mut self, button: PanelButton) -> bool {
        match button {
            PanelButton::Trk | PanelButton::Ptn => {
                let hold = if button == PanelButton::Trk {
                    Hold::Trk
                } else {
                    Hold::Ptn
                };
                // §0 A9 supersedes D6's original "same key toggles off"
                // wording: OS auto-repeat streams Press events for a
                // still-held key, indistinguishable from a deliberate
                // second tap without kitty release events — so re-pressing
                // the SAME armed prefix is now a no-op.
                self.armed = Some(hold);
                true
            }
            _ if trig_col(button).is_some() => {
                // A completed chord disarms (one-shot).
                self.armed = None;
                false
            }
            _ => {
                // D6: any other key disarms and is then processed normally.
                self.armed = None;
                false
            }
        }
    }

    /// Esc disarms unconditionally (D6).
    pub fn on_esc(&mut self) {
        self.armed = None;
    }
}

/// The subset of live app state `button_to_action` needs — decoupled from
/// the full `Model` so this stays pure/testable without a terminal or
/// engine state (D12).
#[derive(Clone, Copy, Debug)]
pub struct ScreenState {
    pub screen: Screen,
    pub grid_rec: bool,
}

/// FUNC (fixed Shift modifier, §0 A15 — not a `PanelButton`) and Ctrl
/// (D8: fine-jog magnitude), as resolved by the caller from a raw
/// `KeyEvent` (see `func_held` for FUNC's case-folding rule).
#[derive(Clone, Copy, Debug, Default)]
pub struct Mods {
    pub func: bool,
    pub ctrl: bool,
}

/// TK2 C2 (D6/D8/D12): resolve a `PanelButton` press to an `Action`, given
/// the current hold-chord state and screen. Pure — no I/O, no engine
/// state.
pub fn button_to_action(
    held: &HeldState,
    screen: &ScreenState,
    button: PanelButton,
    mods: Mods,
) -> Action {
    // D6/A10: an armed TRK/PTN prefix chords with any trig, taking
    // precedence over everything else while armed.
    if let (Some(hold), Some(col)) = (held.armed, trig_col(button)) {
        return match hold {
            Hold::Trk => Action::SelectTrack(col),
            Hold::Ptn => Action::SelectPattern(col),
        };
    }

    // D8/A10: encoder jog resolves only with no armed prefix. Top row
    // (col < 8) is "up"; bottom row is the same encoder index, "down".
    if held.armed.is_none() && mods.func {
        if let Some(col) = trig_col(button) {
            let dir = if col < 8 { Dir::Next } else { Dir::Prev };
            let mag = if mods.ctrl { Mag::Fine } else { Mag::Normal };
            return Action::EncoderJog {
                col: col % 8,
                dir,
                mag,
            };
        }
    }

    if let Some(col) = trig_col(button) {
        // D12: grid_rec defaults on (TK1 behavior preserved); off routes
        // every trig to a live trig (TK2 C1's CMD_TRIG_NOW) instead.
        return if screen.grid_rec {
            Action::ToggleStep { col }
        } else {
            Action::LiveTrig { col }
        };
    }

    match button {
        // A12 (deferred): FUNC+Space (Play) must stay a no-op once Play
        // gains real meaning here (C3+ transport wiring) — Space is a
        // transport-only alias, never the destructive-clear chord. Nothing
        // to guard yet: bare Play already falls through to Noop below, so
        // a dedicated arm here would be unfalsifiable (review finding,
        // post-C2 hostile review) until Play resolves to something else.
        PanelButton::Rec => Action::ToggleGridRec,
        PanelButton::Song => Action::OpenScreen(Screen::Chain),
        PanelButton::Mute => Action::OpenScreen(Screen::Mute),
        PanelButton::Tempo => Action::OpenScreen(Screen::Tempo),
        PanelButton::Settings => Action::OpenScreen(Screen::Settings),
        _ => Action::Noop,
    }
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
            assert!(matches!(map_key(mode, &key('q')), Action::SelectTrack(0)));
            assert!(matches!(map_key(mode, &key('p')), Action::SelectTrack(7)));
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
            map_key(Mode::Seq, &key('z')),
            Action::ToggleStep { col: 8 }
        ));
        assert!(matches!(
            map_key(Mode::Seq, &key('/')),
            Action::ToggleStep { col: 15 }
        ));
    }

    #[test]
    fn step_keys_invariant_in_perf() {
        assert!(matches!(
            map_key(Mode::Perf, &key('a')),
            Action::ToggleStep { col: 0 }
        ));
        assert!(matches!(
            map_key(Mode::Perf, &key(',')),
            Action::ToggleStep { col: 13 }
        ));
        assert!(matches!(
            map_key(Mode::Perf, &key('z')),
            Action::ToggleStep { col: 8 }
        ));
    }

    #[test]
    fn minus_equals_navigate_pages() {
        assert!(matches!(
            map_key(Mode::Seq, &key('-')),
            Action::PageWindow(Dir::Prev)
        ));
        assert!(matches!(
            map_key(Mode::Seq, &key('=')),
            Action::PageWindow(Dir::Next)
        ));
    }

    #[test]
    fn bracket_keys_are_noop() {
        assert!(matches!(map_key(Mode::Seq, &key('[')), Action::Noop));
        assert!(matches!(map_key(Mode::Seq, &key(']')), Action::Noop));
        assert!(matches!(map_key(Mode::Seq, &key('{')), Action::Noop));
        assert!(matches!(map_key(Mode::Seq, &key('}')), Action::Noop));
    }

    #[test]
    fn unknown_key_is_noop() {
        assert!(matches!(map_key(Mode::Seq, &key('0')), Action::Noop));
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

    #[test]
    fn perf_page_select_keys() {
        assert!(matches!(
            map_key(Mode::Perf, &key('1')),
            Action::SelectParamPage(0)
        ));
        assert!(matches!(
            map_key(Mode::Perf, &key('3')),
            Action::SelectParamPage(2)
        ));
        assert!(matches!(
            map_key(Mode::Perf, &key('6')),
            Action::SelectParamPage(5)
        ));
    }

    #[test]
    fn perf_jog_keys() {
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        let fine_up = KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT);

        assert!(matches!(
            map_key(Mode::Perf, &up),
            Action::Jog {
                slot: Slot::A,
                dir: Dir::Next,
                mag: Mag::Normal
            }
        ));
        assert!(matches!(
            map_key(Mode::Perf, &down),
            Action::Jog {
                slot: Slot::A,
                dir: Dir::Prev,
                mag: Mag::Normal
            }
        ));
        assert!(matches!(
            map_key(Mode::Perf, &right),
            Action::Jog {
                slot: Slot::B,
                dir: Dir::Next,
                mag: Mag::Normal
            }
        ));
        assert!(matches!(
            map_key(Mode::Perf, &left),
            Action::Jog {
                slot: Slot::B,
                dir: Dir::Prev,
                mag: Mag::Normal
            }
        ));
        assert!(matches!(
            map_key(Mode::Perf, &fine_up),
            Action::Jog {
                slot: Slot::A,
                dir: Dir::Next,
                mag: Mag::Fine
            }
        ));
    }

    // ── TK2 C2: panel model (pure types + mapping) ───────────────────────

    fn default_grid() -> ScreenState {
        ScreenState {
            screen: Screen::Grid,
            grid_rec: true,
        }
    }

    /// `func_held` is the code that actually implements §0 A1 (rated a
    /// blocker in the hostile review), but every other C2 test exercises
    /// FUNC by hand-constructing `Mods{func:true,...}`, bypassing it
    /// entirely — a regression here would ship untested (review finding,
    /// post-C2 hostile review). Tests it directly against the three input
    /// shapes A1 names: legacy Shift+letter, kitty alternate-keys
    /// Shift+letter (SHIFT cleared, letter uppercase), and a plain key.
    #[test]
    fn func_held_case_folds_and_infers_from_letter_case() {
        // Legacy terminal: uppercase char AND the SHIFT flag.
        assert!(func_held(&KeyEvent::new(
            KeyCode::Char('Q'),
            KeyModifiers::SHIFT
        )));
        // Kitty alternate-keys: uppercase char, SHIFT flag cleared.
        assert!(func_held(&KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::NONE)));
        // Plain lowercase, no modifier: FUNC not held.
        assert!(!func_held(&key('q')));
        // A non-letter with SHIFT still set (a modifier key like Tab):
        // SHIFT alone is sufficient when case-folding doesn't apply.
        assert!(func_held(&KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT)));
        // §0 A1's carve-out: shifted punctuation arrives as the shifted
        // symbol with no SHIFT flag — must NOT be inferred as FUNC (that
        // class is handled separately; A1 says FUNC+digit chords are
        // dropped entirely, not silently treated as held).
        assert!(!func_held(&key('!')));
    }

    #[test]
    fn continuous_grid_maps_sixteen_trigs() {
        let keymap = Keymap::default();
        for (i, &c) in TOP_TRIG_ROW.iter().enumerate() {
            assert_eq!(
                key_to_button(&keymap, key(c)),
                trig_button(i),
                "top row {c:?} must map to Trig{}",
                i + 1
            );
        }
        for (i, &c) in BOTTOM_TRIG_ROW.iter().enumerate() {
            assert_eq!(
                key_to_button(&keymap, key(c)),
                trig_button(8 + i),
                "bottom row {c:?} must map to Trig{}",
                8 + i + 1
            );
        }
    }

    #[test]
    fn trk_hold_plus_trig_selects_track() {
        let held = HeldState {
            kitty: false,
            armed: Some(Hold::Trk),
            pressed: HashSet::new(),
        };
        let action = button_to_action(&held, &default_grid(), PanelButton::Trig5, Mods::default());
        assert!(matches!(action, Action::SelectTrack(4)));
    }

    #[test]
    fn ptn_hold_plus_trig_selects_pattern() {
        let held = HeldState {
            kitty: false,
            armed: Some(Hold::Ptn),
            pressed: HashSet::new(),
        };
        let action = button_to_action(&held, &default_grid(), PanelButton::Trig3, Mods::default());
        assert!(matches!(action, Action::SelectPattern(2)));
    }

    #[test]
    fn sticky_prefix_one_shot_then_disarms() {
        let mut held = HeldState::new(false);
        held.on_press(PanelButton::Trk);
        assert_eq!(held.armed, Some(Hold::Trk));
        held.on_press(PanelButton::Trig1);
        assert_eq!(
            held.armed, None,
            "a trig chord is one-shot: it disarms the prefix"
        );
    }

    #[test]
    fn sticky_prefix_same_key_is_a_noop_per_a9() {
        // §0 A9 supersedes D6's original "same key toggles off" wording:
        // OS auto-repeat streams Press events for a still-held key,
        // indistinguishable (without kitty release events) from a
        // deliberate second tap — so re-pressing the SAME armed prefix is
        // now a no-op, not a toggle.
        let mut held = HeldState::new(false);
        held.on_press(PanelButton::Trk);
        held.on_press(PanelButton::Trk);
        assert_eq!(
            held.armed,
            Some(Hold::Trk),
            "§0 A9: a repeated same-prefix press is a no-op, not a toggle-off"
        );
    }

    /// Named `..._toggles_off` to match the TK2 C2 spec's literal test
    /// list (`design/phases/tk2-theotokos.md` §3) — kept findable under
    /// that name even though §0 A9 rewrote the behavior it verifies to the
    /// opposite of "toggle off" (review finding, post-C2 hostile review:
    /// the name-only-matches-spec-text version was flagged as misleading
    /// on its own). See `sticky_prefix_same_key_is_a_noop_per_a9` for the
    /// accurately-named twin.
    #[test]
    fn sticky_prefix_same_key_toggles_off() {
        sticky_prefix_same_key_is_a_noop_per_a9();
    }

    #[test]
    fn sticky_prefix_esc_disarms() {
        let mut held = HeldState::new(false);
        held.on_press(PanelButton::Ptn);
        held.on_esc();
        assert_eq!(held.armed, None);
    }

    #[test]
    fn nontrig_key_disarms_and_processes() {
        let mut held = HeldState::new(false);
        held.on_press(PanelButton::Trk);
        let consumed = held.on_press(PanelButton::Play);
        assert_eq!(held.armed, None, "a non-trig, non-prefix key disarms");
        assert!(!consumed, "and is still processed normally (not swallowed)");
    }

    #[test]
    fn func_top_row_is_encoder_up_bottom_row_down() {
        let held = HeldState::new(false);
        let mods = Mods {
            func: true,
            ctrl: false,
        };
        let up = button_to_action(&held, &default_grid(), PanelButton::Trig1, mods);
        assert!(matches!(
            up,
            Action::EncoderJog {
                col: 0,
                dir: Dir::Next,
                mag: Mag::Normal
            }
        ));
        let down = button_to_action(&held, &default_grid(), PanelButton::Trig9, mods);
        assert!(matches!(
            down,
            Action::EncoderJog {
                col: 0,
                dir: Dir::Prev,
                mag: Mag::Normal
            }
        ));
        let fine = button_to_action(
            &held,
            &default_grid(),
            PanelButton::Trig1,
            Mods {
                func: true,
                ctrl: true,
            },
        );
        assert!(matches!(
            fine,
            Action::EncoderJog {
                mag: Mag::Fine,
                ..
            }
        ));
    }

    #[test]
    fn rec_toggles_grid_recording() {
        let held = HeldState::new(false);
        let action = button_to_action(&held, &default_grid(), PanelButton::Rec, Mods::default());
        assert!(matches!(action, Action::ToggleGridRec));
    }

    #[test]
    fn trig_with_grid_rec_off_is_live_trig() {
        let held = HeldState::new(false);
        let screen = ScreenState {
            screen: Screen::Grid,
            grid_rec: false,
        };
        let action = button_to_action(&held, &screen, PanelButton::Trig3, Mods::default());
        assert!(matches!(action, Action::LiveTrig { col: 2 }));
    }

    /// Old TK1 actions are unmapped; the keys resolve to their new buttons
    /// (§0 A13's respec of this test).
    #[test]
    fn removed_tk1_bindings_are_dead() {
        let keymap = Keymap::default();
        // 'y' used to be Yank in TK1; the continuous grid claims it as Trig6.
        assert_eq!(key_to_button(&keymap, key('y')), Some(PanelButton::Trig6));
        // Tab used to cycle Mode; it is now the TRK hold prefix.
        assert_eq!(
            key_to_button(&keymap, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Some(PanelButton::Trk)
        );
        // '\\' used to be the leader prefix; the leader is retired outright.
        assert_eq!(key_to_button(&keymap, key('\\')), None);
        // '1' used to select a pattern (Seq mode); it is now page-select.
        assert_eq!(key_to_button(&keymap, key('1')), Some(PanelButton::Pg1));
        // Shift+track (old mute chord) is gone; 'q' with SHIFT case-folds
        // to the same Trig1 identity as plain 'q' (§0 A1) — FUNC is
        // resolved separately by the caller, not by key_to_button.
        assert_eq!(
            key_to_button(&keymap, KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT)),
            Some(PanelButton::Trig1)
        );
    }
}
