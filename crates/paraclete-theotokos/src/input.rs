use std::collections::{BTreeMap, HashMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::action::Action;
use crate::model::{Dir, Mag, RecMode, Screen};

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
    /// TK2.1 C5a (D9): toggles ENC mode (default key `n`).
    Enc,
    /// TK2.1 C5b (D15): the shared p-lock target button (default key `m`
    /// — free since `Mute`'s default binding moved off it this same
    /// commit; C6 formally retires the `Mute` screen/button per D12).
    Lock,
}

/// TK2 C8 (D11): the `:bind`/`:unbind` verb vocabulary — every `PanelButton`
/// variant name, case-insensitive. Single source of truth for both
/// directions (`button_name`/`button_from_name`) so the table can't drift.
const BUTTON_NAMES: &[(&str, PanelButton)] = &[
    ("Trig1", PanelButton::Trig1),
    ("Trig2", PanelButton::Trig2),
    ("Trig3", PanelButton::Trig3),
    ("Trig4", PanelButton::Trig4),
    ("Trig5", PanelButton::Trig5),
    ("Trig6", PanelButton::Trig6),
    ("Trig7", PanelButton::Trig7),
    ("Trig8", PanelButton::Trig8),
    ("Trig9", PanelButton::Trig9),
    ("Trig10", PanelButton::Trig10),
    ("Trig11", PanelButton::Trig11),
    ("Trig12", PanelButton::Trig12),
    ("Trig13", PanelButton::Trig13),
    ("Trig14", PanelButton::Trig14),
    ("Trig15", PanelButton::Trig15),
    ("Trig16", PanelButton::Trig16),
    ("Trk", PanelButton::Trk),
    ("Ptn", PanelButton::Ptn),
    ("Rec", PanelButton::Rec),
    ("Play", PanelButton::Play),
    ("Stop", PanelButton::Stop),
    ("Pg1", PanelButton::Pg1),
    ("Pg2", PanelButton::Pg2),
    ("Pg3", PanelButton::Pg3),
    ("Pg4", PanelButton::Pg4),
    ("Pg5", PanelButton::Pg5),
    ("Pg6", PanelButton::Pg6),
    ("Kit", PanelButton::Kit),
    ("Settings", PanelButton::Settings),
    ("Sampling", PanelButton::Sampling),
    ("Tempo", PanelButton::Tempo),
    ("Yes", PanelButton::Yes),
    ("No", PanelButton::No),
    ("Up", PanelButton::Up),
    ("Down", PanelButton::Down),
    ("Left", PanelButton::Left),
    ("Right", PanelButton::Right),
    ("PagePrev", PanelButton::PagePrev),
    ("PageNext", PanelButton::PageNext),
    ("Song", PanelButton::Song),
    ("Keybd", PanelButton::Keybd),
    ("Enc", PanelButton::Enc),
    ("Lock", PanelButton::Lock),
];

/// TK2 C8 (D11): canonical name for a `PanelButton`, as written by
/// `:list-bindings` and `Keymap::to_yaml`.
pub fn button_name(button: PanelButton) -> &'static str {
    BUTTON_NAMES
        .iter()
        .find(|(_, b)| *b == button)
        .map(|(name, _)| *name)
        .unwrap_or("?")
}

/// TK2 C8 (D11): the inverse of `button_name`, case-insensitive — the
/// `:bind`/`:unbind` verbs and `Keymap::from_yaml` both resolve button
/// names through this.
pub fn button_from_name(name: &str) -> Option<PanelButton> {
    let lower = name.trim().to_lowercase();
    BUTTON_NAMES
        .iter()
        .find(|(n, _)| n.to_lowercase() == lower)
        .map(|(_, b)| *b)
}

/// TK2 C8 (D14/§0 A6): keys the `:bind` verb refuses to rebind. Ctrl-C
/// (quit) is unbindable structurally — `lib.rs::handle_keys` intercepts it
/// as a `direct_action` before the keymap is ever consulted, regardless of
/// what `c` alone is bound to — so only the `:` line's own key needs a
/// guard here (§0 A6: "the D14 unbindable entry is `Char(':')`").
pub fn is_unbindable(code: KeyCode) -> bool {
    matches!(code, KeyCode::Char(':'))
}

/// TK2 C8: parses a `:bind`/`:unbind` key token (and `Keymap::from_yaml`'s
/// map keys) into a `KeyCode`. Single-character tokens map directly
/// (case-folded, matching `normalize_code`); everything else is a named
/// token (`tab`, `esc`, `f1`...`f24`, arrows, ...), case-insensitive. The
/// inverse of `key_name`.
pub fn key_from_name(name: &str) -> Option<KeyCode> {
    let lower = name.trim().to_lowercase();
    if lower.chars().count() == 1 {
        return lower.chars().next().map(KeyCode::Char);
    }
    match lower.as_str() {
        "space" => Some(KeyCode::Char(' ')),
        "tab" => Some(KeyCode::Tab),
        "backtab" => Some(KeyCode::BackTab),
        "enter" | "return" => Some(KeyCode::Enter),
        "esc" | "escape" => Some(KeyCode::Esc),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "backspace" => Some(KeyCode::Backspace),
        "delete" | "del" => Some(KeyCode::Delete),
        "insert" | "ins" => Some(KeyCode::Insert),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" | "pgup" => Some(KeyCode::PageUp),
        "pagedown" | "pgdn" => Some(KeyCode::PageDown),
        "null" => Some(KeyCode::Null),
        _ if lower.starts_with('f') => lower[1..].parse::<u8>().ok().map(KeyCode::F),
        _ => None,
    }
}

/// TK2 C8: the inverse of `key_from_name` — the canonical token
/// `:list-bindings` and `Keymap::to_yaml` write for a `KeyCode`.
pub fn key_name(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_ascii_lowercase().to_string(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => "backtab".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Insert => "insert".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        KeyCode::Null => "null".into(),
        KeyCode::F(n) => format!("f{n}"),
        _ => "?".into(),
    }
}

/// `col` 0..16 → the matching `PanelButton::TrigN`. `pub`: lib.rs and
/// render.rs (TK2.1 C2) both need to enumerate all 16 trig buttons to
/// resolve `key_label` for the trig strip's chip column.
pub fn trig_button(col: usize) -> Option<PanelButton> {
    use PanelButton::*;
    const TABLE: [PanelButton; 16] = [
        Trig1, Trig2, Trig3, Trig4, Trig5, Trig6, Trig7, Trig8, Trig9, Trig10, Trig11, Trig12,
        Trig13, Trig14, Trig15, Trig16,
    ];
    TABLE.get(col).copied()
}

/// The inverse of `trig_button`: `None` for any non-trig button. `pub`:
/// `lib.rs` needs it to gate the repeat-consumption guard on trig buttons
/// (TK2.2 C1, BUG-046).
pub fn trig_col(button: PanelButton) -> Option<usize> {
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

/// The continuous grid's top row (§2): `q w e r t y u i` → Trig1..8. TK2.1
/// C2: `built_in_button` reads `DEFAULT_BINDINGS` now, not these — kept
/// `#[cfg(test)]` as a from-the-spec fixture `continuous_grid_maps_sixteen_trigs`
/// checks `DEFAULT_BINDINGS`/`key_to_button` against.
#[cfg(test)]
const TOP_TRIG_ROW: [char; 8] = ['q', 'w', 'e', 'r', 't', 'y', 'u', 'i'];
/// The continuous grid's bottom row (§2): `a s d f g h j k` → Trig9..16.
#[cfg(test)]
const BOTTOM_TRIG_ROW: [char; 8] = ['a', 's', 'd', 'f', 'g', 'h', 'j', 'k'];

/// TK2.1 C2 (D3): the §2 panel table as data — the single source both
/// `built_in_button` (key → button) and `key_label` (button → key, for
/// chip/legend rendering) read, so the two directions cannot drift. The
/// `bool` marks the **preferred** key for a button reachable by more than
/// one (only `Play`: `x` and `Space` both reach it, but the chip must read
/// `[x] PLAY`, not whichever key sorts first lexicographically).
const DEFAULT_BINDINGS: &[(KeyCode, PanelButton, bool)] = &[
    (KeyCode::Char('q'), PanelButton::Trig1, true),
    (KeyCode::Char('w'), PanelButton::Trig2, true),
    (KeyCode::Char('e'), PanelButton::Trig3, true),
    (KeyCode::Char('r'), PanelButton::Trig4, true),
    (KeyCode::Char('t'), PanelButton::Trig5, true),
    (KeyCode::Char('y'), PanelButton::Trig6, true),
    (KeyCode::Char('u'), PanelButton::Trig7, true),
    (KeyCode::Char('i'), PanelButton::Trig8, true),
    (KeyCode::Char('a'), PanelButton::Trig9, true),
    (KeyCode::Char('s'), PanelButton::Trig10, true),
    (KeyCode::Char('d'), PanelButton::Trig11, true),
    (KeyCode::Char('f'), PanelButton::Trig12, true),
    (KeyCode::Char('g'), PanelButton::Trig13, true),
    (KeyCode::Char('h'), PanelButton::Trig14, true),
    (KeyCode::Char('j'), PanelButton::Trig15, true),
    (KeyCode::Char('k'), PanelButton::Trig16, true),
    (KeyCode::Tab, PanelButton::Trk, true),
    (KeyCode::Char('p'), PanelButton::Ptn, true),
    (KeyCode::Char('z'), PanelButton::Rec, true),
    (KeyCode::Char('x'), PanelButton::Play, true),
    // A12/A16: `Space` is a PLAY alias only — resolved as a transport-only
    // no-op under FUNC by `button_to_action`, not here.
    (KeyCode::Char(' '), PanelButton::Play, false),
    (KeyCode::Char('c'), PanelButton::Stop, true),
    (KeyCode::Char('1'), PanelButton::Pg1, true),
    (KeyCode::Char('2'), PanelButton::Pg2, true),
    (KeyCode::Char('3'), PanelButton::Pg3, true),
    (KeyCode::Char('4'), PanelButton::Pg4, true),
    (KeyCode::Char('5'), PanelButton::Pg5, true),
    (KeyCode::Char('6'), PanelButton::Pg6, true),
    (KeyCode::Char('7'), PanelButton::Kit, true),
    (KeyCode::Char('8'), PanelButton::Settings, true),
    (KeyCode::Char('9'), PanelButton::Sampling, true),
    (KeyCode::Char('0'), PanelButton::Tempo, true),
    (KeyCode::Enter, PanelButton::Yes, true),
    (KeyCode::Esc, PanelButton::No, true),
    (KeyCode::Up, PanelButton::Up, true),
    (KeyCode::Down, PanelButton::Down, true),
    (KeyCode::Left, PanelButton::Left, true),
    (KeyCode::Right, PanelButton::Right, true),
    (KeyCode::Char('-'), PanelButton::PagePrev, true),
    (KeyCode::Char('='), PanelButton::PageNext, true),
    (KeyCode::Char('o'), PanelButton::Song, true),
    // TK2.1 C5b: 'm' moves off Mute (no default key of its own until a
    // user `:bind`s one — Mute's screen/button are formally retired in
    // C6, per D12) onto Lock, the shared p-lock target button.
    (KeyCode::Char('v'), PanelButton::Keybd, true),
    (KeyCode::Char('n'), PanelButton::Enc, true),
    (KeyCode::Char('m'), PanelButton::Lock, true),
];

/// A normalized key for the user keymap (D11) and the built-in §2 table:
/// `Char` letters are always lowercase — see `func_held` (§0 A1), which
/// carries the case-implied FUNC bit separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyBinding {
    pub code: KeyCode,
}

/// The user keymap (D11): flat, global, no per-screen bindings. Empty by
/// default — C2 introduced the type; C8 adds YAML load/save + the `:bind`
/// family of verbs.
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    pub bindings: HashMap<KeyBinding, PanelButton>,
}

impl Keymap {
    /// TK2 C8 (D11): global config path, `~/.config/paraclete/keymap.yaml`.
    /// `None` if `$HOME` isn't set (headless/CI — `load_startup` just skips
    /// the global source in that case).
    pub fn global_path() -> Option<std::path::PathBuf> {
        std::env::var_os("HOME").map(|home| {
            std::path::Path::new(&home)
                .join(".config")
                .join("paraclete")
                .join("keymap.yaml")
        })
    }

    /// TK2 C8 (D11): serializes to the flat `key: Button` YAML map (keys
    /// via `key_name`, buttons via `button_name`) — sorted (`BTreeMap`) so
    /// output is deterministic across runs, not tied to `HashMap` order.
    pub fn to_yaml(&self) -> Result<String, String> {
        let map: BTreeMap<String, String> = self
            .bindings
            .iter()
            .map(|(k, v)| (key_name(k.code), button_name(*v).to_string()))
            .collect();
        serde_yml::to_string(&map).map_err(|e| e.to_string())
    }

    /// TK2 C8 (D11)/TK2.1 C6 (D14): the inverse of `to_yaml`. Structurally
    /// invalid YAML, or an entry targeting an unbindable key (`:` — D14),
    /// still fails the whole parse (a malformed or tampered hand-edited
    /// file smuggling a `:` binding should surface loudly, not silently —
    /// enforcing `is_unbindable` here, not just in the `:bind` verb parser,
    /// closes a hand-edited-YAML loophole found in post-C8 hostile
    /// review). TK2.1 C6 (D14) softens the other two failure modes: an
    /// entry naming an unrecognized key or button (e.g. a stale `m: Mute`
    /// line after Mute's retirement) no longer rejects the whole file —
    /// it's skipped, the rest of the file loads, and the skipped `"key:
    /// button"` entries are returned for the caller to report (previously
    /// one stale line would reject a user's entire keymap).
    pub fn from_yaml(text: &str) -> Result<(Self, Vec<String>), String> {
        let map: BTreeMap<String, String> = serde_yml::from_str(text).map_err(|e| e.to_string())?;
        let mut bindings = HashMap::with_capacity(map.len());
        let mut skipped = Vec::new();
        for (key_str, button_str) in map {
            let code = match key_from_name(&key_str) {
                Some(c) => c,
                None => {
                    skipped.push(format!("{key_str}: {button_str}"));
                    continue;
                }
            };
            if is_unbindable(code) {
                return Err(format!("{key_str} is reserved and cannot be bound"));
            }
            let button = match button_from_name(&button_str) {
                Some(b) => b,
                None => {
                    skipped.push(format!("{key_str}: {button_str}"));
                    continue;
                }
            };
            bindings.insert(
                KeyBinding {
                    code: normalize_code(code),
                },
                button,
            );
        }
        Ok((Keymap { bindings }, skipped))
    }

    /// TK2 C8 (D11): merges two already-parsed YAML sources in load order —
    /// global first, then local (local wins on key collision, via
    /// `HashMap::extend`'s overwrite semantics). Malformed (structurally
    /// invalid, or containing an unbindable-key entry) sources are skipped
    /// entirely, not fatal: a broken `keymap.yaml` should degrade to
    /// built-in defaults, not block startup. Split out from `load_startup`
    /// so the merge policy is testable without touching the filesystem
    /// (`local_file_overrides_global`). Returns the merged keymap plus
    /// every skipped-entry description (D14) across both sources, in load
    /// order.
    pub fn merge_sources(global: Option<&str>, local: Option<&str>) -> (Self, Vec<String>) {
        let mut merged = Keymap::default();
        let mut skipped = Vec::new();
        for text in [global, local].into_iter().flatten() {
            if let Ok((loaded, mut sk)) = Keymap::from_yaml(text) {
                merged.bindings.extend(loaded.bindings);
                skipped.append(&mut sk);
            }
        }
        (merged, skipped)
    }

    /// TK2 C8 (D11): startup load order — global
    /// (`~/.config/paraclete/keymap.yaml`) then local (`./keymap.yaml`,
    /// "still overrides on load"). Missing files are not errors (most
    /// users have neither); this is the only place `:load-bindings` and
    /// `TheotokosApp::new` need to call.
    pub fn load_startup() -> (Self, Vec<String>) {
        let global = Self::global_path().and_then(|p| std::fs::read_to_string(p).ok());
        let local = std::fs::read_to_string("keymap.yaml").ok();
        Self::merge_sources(global.as_deref(), local.as_deref())
    }

    /// TK2 C8 (D14): the only write path — `:save-bindings` is the sole
    /// caller (no auto-save on quit or elsewhere). Always targets the
    /// global path; the local `./keymap.yaml` override is read-only from
    /// the app's perspective (hand-authored, e.g. a per-project preset).
    pub fn save_global(&self) -> Result<(), String> {
        let path = Self::global_path().ok_or("cannot resolve $HOME to save bindings")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let yaml = self.to_yaml()?;
        std::fs::write(&path, yaml).map_err(|e| e.to_string())
    }
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
/// `pub(crate)`: TK2 C8's `:bind`/`:unbind` dispatch (`lib.rs`) constructs
/// `KeyBinding`s directly and must normalize the same way `key_to_button`
/// does, or a bind of `Q` (uppercase) would silently never match a lookup
/// keyed on lowercase `q`.
pub(crate) fn normalize_code(code: KeyCode) -> KeyCode {
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

/// TK2.1 C2 (D3): reads `DEFAULT_BINDINGS` — the same table `key_label`
/// reads in the reverse direction, so the two cannot drift.
fn built_in_button(code: KeyCode) -> Option<PanelButton> {
    DEFAULT_BINDINGS
        .iter()
        .find(|(k, _, _)| *k == code)
        .map(|(_, b, _)| *b)
}

/// TK2.1 C2 (D3): the key chip label for a `PanelButton`, resolved through
/// the live `Keymap` exactly as `key_to_button` resolves the forward
/// direction — a user binding wins (lowest `key_name` among several, for
/// determinism if a button somehow gained two user bindings); otherwise
/// the preferred `DEFAULT_BINDINGS` key, **but only if no user binding has
/// claimed that `KeyCode` for a different button** (shadow-awareness:
/// `key_to_button` consults user bindings first, so a shadowed default key
/// no longer reaches this button). Returns the lowercase storage form —
/// title-casing multi-character names for display (`[Tab]`, not `[tab]`)
/// is the caller's job (render.rs), matching D3's "chip casing is
/// display-only".
pub fn key_label(keymap: &Keymap, button: PanelButton) -> Option<String> {
    let mut user_keys: Vec<KeyCode> = keymap
        .bindings
        .iter()
        .filter(|(_, b)| **b == button)
        .map(|(k, _)| k.code)
        .collect();
    if !user_keys.is_empty() {
        user_keys.sort_by_key(|c| key_name(*c));
        return Some(key_name(user_keys[0]));
    }

    let (default_code, _, _) = DEFAULT_BINDINGS
        .iter()
        .find(|(_, b, preferred)| *b == button && *preferred)?;
    let shadowed = keymap.bindings.contains_key(&KeyBinding {
        code: *default_code,
    });
    if shadowed {
        None
    } else {
        Some(key_name(*default_code))
    }
}

/// D6: which hold-prefix is armed. TK2.1 C1 (D5) adds `Rec` — kitty path
/// only, so REC held + PLAY can resolve to `Action::EnterLiveRec`; the
/// sticky fallback (`on_press`) never arms it (REC's own action fires on
/// every press regardless, so unlike `Trk`/`Ptn` it never needs to wait
/// for a release-less approximation of "held"). TK2.1 C5b (D15) adds
/// `Lock` — arms on both paths exactly like `Trk`/`Ptn` (Lock's own press
/// does nothing until a trig consumes it, so it's safe to consume/arm
/// unconditionally); the "press Lock again to clear an already-set
/// target" case is intercepted in `lib.rs::handle_keys` *before* this
/// arming even runs, since that decision needs `Model.lock_target`, which
/// `HeldState` deliberately doesn't know about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    Trk,
    Ptn,
    Rec,
    Lock,
}

/// TK2.1 C6 (D11): how close (in ms) two same-prefix sticky presses must
/// land to be treated as one OS auto-repeat stream rather than two
/// deliberate taps. *(tunable)*
const REPEAT_GUARD_MS: u128 = 400;

/// D6 hold-chord state, both branches: `kitty = true` selects the
/// real-hold path (press arms, physical release disarms — wired to
/// crossterm release events in C3); `kitty = false` (the common case,
/// probed via `supports_keyboard_enhancement()` at startup) selects the
/// one-shot sticky fallback this struct implements today. TK2.1 C6 (D11)
/// reverses §0 A9's "repeated same-prefix press is a no-op": a genuine
/// re-tap (outside `REPEAT_GUARD_MS` of the previous same-prefix press)
/// now disarms, since without kitty release events that's the only
/// "press it again to cancel" gesture available; a press *inside* the
/// guard window is still treated as OS auto-repeat and ignored.
#[derive(Debug, Default)]
pub struct HeldState {
    pub kitty: bool,
    pub armed: Option<Hold>,
    /// Physically-pressed panel buttons, kitty mode only — tracked so a
    /// release event can tell which prefix to drop. Unused by the sticky
    /// fallback below; wired alongside kitty release handling in C3.
    pub pressed: HashSet<PanelButton>,
    /// TK2.1 C6 (D11): which prefix armed on the most recent sticky-path
    /// press, and when — lets a same-prefix re-press tell a deliberate
    /// second tap (a real gap) from an OS auto-repeat pulse (clustered
    /// within `REPEAT_GUARD_MS`).
    last_prefix_press: Option<(Hold, std::time::Instant)>,
}

impl HeldState {
    pub fn new(kitty: bool) -> Self {
        Self {
            kitty,
            armed: None,
            pressed: HashSet::new(),
            last_prefix_press: None,
        }
    }

    /// D6 (sticky fallback) + TK2.1 C6 (D11): process one button PRESS,
    /// timestamped by the caller so auto-repeat clustering can be judged
    /// against `REPEAT_GUARD_MS`. Returns `true` if the press was consumed
    /// by prefix arm/disarm bookkeeping itself (the caller must not also
    /// resolve an `Action` for it, since `Trk`/`Ptn` on their own are not
    /// actions); `false` means the caller should resolve the button
    /// normally via `button_to_action` — reading `self.armed` as it stood
    /// *before* this call, since a completed chord disarms as a side
    /// effect of the same press.
    pub fn on_press(&mut self, button: PanelButton, now: std::time::Instant) -> bool {
        match button {
            PanelButton::Trk | PanelButton::Ptn | PanelButton::Lock => {
                let hold = match button {
                    PanelButton::Trk => Hold::Trk,
                    PanelButton::Ptn => Hold::Ptn,
                    _ => Hold::Lock,
                };
                if self.armed == Some(hold) {
                    // D11: already armed by this same prefix — a press
                    // clustered within the guard window is OS auto-repeat
                    // (ignored, stays armed); a press further out is a
                    // deliberate re-tap (disarms). TK2.1 C5b: the "press
                    // Lock again clears an already-set target" case is
                    // intercepted by the caller before this runs (see
                    // `Hold`'s doc comment), so Lock only ever reaches
                    // this branch while merely *pending* — the same
                    // guard/disarm logic still applies to that arm.
                    let is_auto_repeat = self.last_prefix_press.is_some_and(|(h, t)| {
                        h == hold && now.duration_since(t).as_millis() < REPEAT_GUARD_MS
                    });
                    self.last_prefix_press = Some((hold, now));
                    if !is_auto_repeat {
                        self.armed = None;
                    }
                } else {
                    self.armed = Some(hold);
                    self.last_prefix_press = Some((hold, now));
                }
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
    ///
    /// Also clears `pressed`, because in the kitty path this is the recovery
    /// from a hold whose release never arrived (BUG-050) — leaving a stale
    /// entry there would put the next press/release pair for that key out of
    /// step, so the first re-tap would disarm nothing.
    pub fn on_esc(&mut self) {
        self.armed = None;
        self.pressed.clear();
    }

    /// Focus left the terminal mid-hold, so the release that would have
    /// disarmed the prefix is never delivered — same recovery as Esc, applied
    /// without waiting for the user to discover they are latched (BUG-050).
    pub fn on_focus_lost(&mut self) {
        self.on_esc();
    }

    /// D6 (kitty branch, TK2 C3): a TRK/PTN press arms for as long as the
    /// key stays physically down — real hold, not one-shot. Returns
    /// `true` if consumed (same contract as `on_press`); non-prefix
    /// buttons are not tracked here (only presence of `armed` matters to
    /// `button_to_action`, so pressed only ever records the hold key
    /// itself). TK2.1 C1 (D5): REC also arms `Hold::Rec` for the duration
    /// of the physical hold, but — unlike TRK/PTN — is **not** consumed:
    /// REC's own `Action::ToggleRec` must still resolve on this same
    /// press, so the caller sees `false` and calls `button_to_action`
    /// normally.
    pub fn on_kitty_press(&mut self, button: PanelButton) -> bool {
        match button {
            PanelButton::Trk | PanelButton::Ptn | PanelButton::Lock => {
                let hold = match button {
                    PanelButton::Trk => Hold::Trk,
                    PanelButton::Ptn => Hold::Ptn,
                    _ => Hold::Lock,
                };
                self.pressed.insert(button);
                self.armed = Some(hold);
                true
            }
            PanelButton::Rec => {
                self.pressed.insert(button);
                self.armed = Some(Hold::Rec);
                false
            }
            _ => false,
        }
    }

    /// D6 (kitty branch): physical release disarms. A release for a button
    /// that was never tracked as pressed (e.g. a trig's release) is a
    /// no-op — only TRK/PTN releases matter here.
    pub fn on_kitty_release(&mut self, button: PanelButton) {
        if self.pressed.remove(&button) {
            self.armed = None;
        }
    }
}

/// The subset of live app state `button_to_action` needs — decoupled from
/// the full `Model` so this stays pure/testable without a terminal or
/// engine state (D12).
#[derive(Clone, Copy, Debug)]
pub struct ScreenState {
    pub screen: Screen,
    pub rec: RecMode,
    /// TK2.1 C5a (D9): explicit encoder-access mode.
    pub enc: bool,
    /// TK2.1 C5b (D15): whether a lock target is currently set **on the
    /// active track**, and if so which step — pre-resolved by the caller
    /// (`lib.rs`) so this stays pure/testable without a `Model` reference.
    pub lock_target_step: Option<usize>,
}

/// FUNC (fixed Shift modifier, §0 A15 — not a `PanelButton`) and Ctrl
/// (D8: fine-jog magnitude), as resolved by the caller from a raw
/// `KeyEvent` (see `func_held` for FUNC's case-folding rule).
#[derive(Clone, Copy, Debug, Default)]
pub struct Mods {
    pub func: bool,
    pub ctrl: bool,
}

/// TK2 C5 (§0 A11): pressing a Pg key while ALREADY on that page cycles
/// its sub-page instead of re-opening it (which would just reset back to
/// sub-page 0) — the §0 A1 hypothesis for reaching params past the first 8.
fn open_or_cycle_sub_page(screen: &ScreenState, idx: usize) -> Action {
    if matches!(screen.screen, Screen::Param(p) if p == idx) {
        Action::NextSubPage
    } else {
        Action::OpenScreen(Screen::Param(idx))
    }
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
    // precedence over everything else while armed. A10: FUNC+trig while
    // TRK held is the mute-toggle chord; while PTN held it has no defined
    // meaning (a no-op, not a wrong-because-legacy pattern select).
    if let (Some(hold), Some(col)) = (held.armed, trig_col(button)) {
        if mods.func {
            return match hold {
                Hold::Trk => Action::ToggleMute(col),
                Hold::Ptn | Hold::Rec | Hold::Lock => Action::Noop,
            };
        }
        return match hold {
            Hold::Trk => Action::SelectTrack(col),
            Hold::Ptn => Action::SelectPattern(col),
            // TK2.1 C1 (D5): REC has no defined trig chord — a bare pad
            // press while REC is physically held (kitty) is not a thing
            // the reference box does; PLAY is REC's only chord partner.
            Hold::Rec => Action::Noop,
            // TK2.1 C5b (D15): the latched path — Lock armed + the next
            // trig sets the lock target. Only meaningful in Grid mode (a
            // "step" only exists there — D6); elsewhere the trig still
            // consumes the arm (matching Trk/Ptn's one-shot precedent)
            // but there is nothing to target.
            Hold::Lock => {
                if screen.rec == RecMode::Grid {
                    Action::SetLockTarget(col)
                } else {
                    Action::Noop
                }
            }
        };
    }

    // D7: while TRK/PTN is held, FUNC+transport (REC/PLAY/STOP) is
    // reserved — a no-op, not the copy/clear/paste chord below.
    if held.armed.is_some()
        && mods.func
        && matches!(
            button,
            PanelButton::Rec | PanelButton::Play | PanelButton::Stop
        )
    {
        return Action::Noop;
    }

    // TK2.1 C5a (D9/A10): ENC mode — a bare trig resolves to an encoder
    // jog on ANY screen, not only Param, while no prefix is armed. Bare =
    // Normal, Ctrl = Fine, FUNC = Coarse (the first producer of
    // `Mag::Coarse`). Checked before the D8 FUNC+trig path below, which
    // stays the (only) way to jog while ENC is off.
    if screen.enc && held.armed.is_none() {
        if let Some(col) = trig_col(button) {
            let dir = if col < 8 { Dir::Next } else { Dir::Prev };
            let mag = if mods.func {
                Mag::Coarse
            } else if mods.ctrl {
                Mag::Fine
            } else {
                Mag::Normal
            };
            return Action::EncoderJog {
                col: col % 8,
                dir,
                mag,
            };
        }
    }

    // D8/A10: outside ENC mode, FUNC+trig is the only way to jog — encoder
    // jog resolves only with no armed prefix. Top row (col < 8) is "up";
    // bottom row is the same encoder index, "down".
    if !screen.enc && held.armed.is_none() && mods.func {
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
        // TK2.1 C5b (D15): re-pressing the trig that set the current lock
        // target clears it (Grid mode only — the latched-arm case above
        // already handled the "not yet set" half of this toggle).
        if screen.rec == RecMode::Grid && screen.lock_target_step == Some(col) {
            return Action::ClearLockTarget;
        }
        // TK2.1 C1 (D5/D6): Grid writes/clears steps of the selected
        // track; Off and Live are pad modes — a trig sounds and selects
        // track N (D6), handled by `Action::LiveTrig` in `lib.rs`.
        return match screen.rec {
            RecMode::Grid => Action::ToggleStep { col },
            RecMode::Off | RecMode::Live => Action::LiveTrig { col },
        };
    }

    match button {
        // TK2.1 C1 (D5): REC held (kitty path only — `held.armed` is only
        // ever `Some(Hold::Rec)` there, per `HeldState::on_press`'s fixed
        // sticky-fallback match) + bare PLAY enters Live and starts the
        // transport. Checked before the plain PLAY arms below so it wins
        // the chord.
        PanelButton::Play if !mods.func && held.armed == Some(Hold::Rec) => {
            Action::EnterLiveRec
        }
        // TK2 C3: Play (bare) restores the transport toggle Space provided
        // in TK1 — the Play button IS the `Space` alias (§2).
        PanelButton::Play if !mods.func => Action::PlayToggle,
        // TK2 C4 (D7): FUNC+PLAY clears the active track's pattern.
        // A12: Space is also the Play alias, so FUNC+Space collapses onto
        // this same arm at the button-identity level — this function
        // cannot tell them apart post-collapse (D11). The A12 no-op for
        // FUNC+Space specifically is enforced one layer up, in
        // `lib.rs::handle_keys`, using the raw key before it reaches here
        // (post-C4 hostile review: this WAS a real gap — FUNC+Space could
        // silently wipe the active pattern).
        PanelButton::Play => Action::ClearLane,
        // TK2.1 C1 (D5): bare REC toggles rec mode — fires on every press
        // regardless of whether this is also the first half of a REC+PLAY
        // chord (see the `Hold::Rec` arm above).
        PanelButton::Rec if !mods.func => Action::ToggleRec,
        // TK2 C4 (D7): FUNC+REC copies the active lane.
        PanelButton::Rec => Action::CopyLane,
        // TK2 C4 (D7): FUNC+STOP pastes.
        PanelButton::Stop if mods.func => Action::PasteLane,
        // ADR-046 T5: bare STOP halts in place and rewinds to the window
        // start — the reference box's "stop", distinct from PLAY's pause.
        PanelButton::Stop => Action::Stop,
        PanelButton::PagePrev => Action::PageWindow(Dir::Prev),
        PanelButton::PageNext => Action::PageWindow(Dir::Next),
        PanelButton::Pg1 => open_or_cycle_sub_page(screen, 0),
        PanelButton::Pg2 => open_or_cycle_sub_page(screen, 1),
        PanelButton::Pg3 => open_or_cycle_sub_page(screen, 2),
        PanelButton::Pg4 => open_or_cycle_sub_page(screen, 3),
        PanelButton::Pg5 => open_or_cycle_sub_page(screen, 4),
        PanelButton::Pg6 => open_or_cycle_sub_page(screen, 5),
        PanelButton::Song => Action::OpenScreen(Screen::Chain),
        PanelButton::Tempo => Action::OpenScreen(Screen::Tempo),
        PanelButton::Settings => Action::OpenScreen(Screen::Settings),
        // TK2 C6 (D12): Tempo screen — YES taps, UP/DOWN nudge bpm (FUNC
        // = fine, ±0.1; bare = ±1).
        PanelButton::Yes if matches!(screen.screen, Screen::Tempo) => Action::TapTempo,
        PanelButton::Up if matches!(screen.screen, Screen::Tempo) => {
            Action::NudgeBpm(if mods.func { 0.1 } else { 1.0 })
        }
        PanelButton::Down if matches!(screen.screen, Screen::Tempo) => {
            Action::NudgeBpm(if mods.func { -0.1 } else { -1.0 })
        }
        // TK2 C6 (D12): Chain screen — YES pushes the cursor pattern,
        // NO clears (Backspace also clears — handled one layer up in
        // `lib.rs`, since Backspace bypasses the button system entirely),
        // LEFT/RIGHT move the bank-row cursor (D13).
        PanelButton::Yes if matches!(screen.screen, Screen::Chain) => Action::ChainPush,
        PanelButton::No if matches!(screen.screen, Screen::Chain) => Action::ChainClear,
        PanelButton::Left if matches!(screen.screen, Screen::Chain) => {
            Action::MoveChainCursor(Dir::Prev)
        }
        PanelButton::Right if matches!(screen.screen, Screen::Chain) => {
            Action::MoveChainCursor(Dir::Next)
        }
        // D12: KIT has no screen in TK2 — echoes reserved.
        PanelButton::Kit => Action::Echo("reserved (kit)"),
        // D12: SAMPLING is hidden entirely unless some capability
        // declares it (none does today) — a plain no-op, not even an
        // echo (unlike KIT).
        PanelButton::Sampling => Action::Noop,
        // §2/D12 name no "return to Grid" gesture anywhere — found live in
        // the TK2 C7 agent smoke pass: Settings/Tempo/Param were dead
        // ends with no specified way back (Mute was too, before its
        // TK2.1 C6 retirement). NO already has a specific
        // meaning on Chain (clear, above, which wins since it's checked
        // first); everywhere else it's unclaimed, so it doubles as the
        // conventional "back" gesture rather than staying a pure no-op.
        // Flagged for design-log follow-up, not treated as a new feature.
        PanelButton::No => Action::OpenScreen(Screen::Grid),
        // TK2.1 C5a (D9): bare ENC toggles the mode; FUNC+Enc has no
        // defined meaning yet (a plain no-op via the catch-all below).
        PanelButton::Enc if !mods.func => Action::ToggleEnc,
        _ => Action::Noop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    // ── TK2 C2: panel model (pure types + mapping) ───────────────────────

    fn default_grid() -> ScreenState {
        ScreenState {
            screen: Screen::Grid,
            rec: RecMode::Grid,
            enc: false,
            lock_target_step: None,
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
            last_prefix_press: None,
        };
        let action = button_to_action(&held, &default_grid(), PanelButton::Trig5, Mods::default());
        assert!(matches!(action, Action::SelectTrack(4)));
    }

    /// A10 reserves FUNC+trig while TRK/PTN is armed for the mute-toggle
    /// chord (TK2 C4, D7) — until that lands, it must be a no-op, not a
    /// wrong-because-legacy `SelectTrack`/`SelectPattern` (post-C3 hostile
    /// review finding).
    #[test]
    fn armed_func_trig_is_reserved_not_a_wrong_select() {
        // TK2 C4 gave TRK+FUNC+trig its real meaning (mute toggle — see
        // `trk_func_trig_toggles_mute`); PTN+FUNC+trig still has none
        // defined, so it must stay a no-op rather than PTN's legacy
        // `SelectPattern`.
        let held = HeldState {
            kitty: false,
            armed: Some(Hold::Ptn),
            pressed: HashSet::new(),
            last_prefix_press: None,
        };
        let mods = Mods {
            func: true,
            ctrl: false,
        };
        let action = button_to_action(&held, &default_grid(), PanelButton::Trig5, mods);
        assert!(
            matches!(action, Action::Noop),
            "FUNC+trig while PTN-armed must not resolve to a pattern select"
        );
    }

    #[test]
    fn ptn_hold_plus_trig_selects_pattern() {
        let held = HeldState {
            kitty: false,
            armed: Some(Hold::Ptn),
            pressed: HashSet::new(),
            last_prefix_press: None,
        };
        let action = button_to_action(&held, &default_grid(), PanelButton::Trig3, Mods::default());
        assert!(matches!(action, Action::SelectPattern(2)));
    }

    /// D7/A10 (TK2 C4): TRK-held + FUNC+trig is the mute-toggle chord —
    /// distinct from bare TRK+trig (track select, tested above).
    #[test]
    fn trk_func_trig_toggles_mute() {
        let held = HeldState {
            kitty: false,
            armed: Some(Hold::Trk),
            pressed: HashSet::new(),
            last_prefix_press: None,
        };
        let mods = Mods {
            func: true,
            ctrl: false,
        };
        let action = button_to_action(&held, &default_grid(), PanelButton::Trig5, mods);
        assert!(matches!(action, Action::ToggleMute(4)));
    }

    /// D7 (TK2 C4): while TRK/PTN is held, FUNC+transport (REC/PLAY/STOP)
    /// is reserved — a no-op, not the copy/clear/paste chord.
    #[test]
    fn func_transport_noop_while_trk_held() {
        let held = HeldState {
            kitty: false,
            armed: Some(Hold::Trk),
            pressed: HashSet::new(),
            last_prefix_press: None,
        };
        let mods = Mods {
            func: true,
            ctrl: false,
        };
        for button in [PanelButton::Rec, PanelButton::Play, PanelButton::Stop] {
            let action = button_to_action(&held, &default_grid(), button, mods);
            assert!(
                matches!(action, Action::Noop),
                "FUNC+{button:?} while TRK held must be a no-op, got {action:?}"
            );
        }
    }

    #[test]
    fn sticky_prefix_one_shot_then_disarms() {
        let mut held = HeldState::new(false);
        let t0 = std::time::Instant::now();
        held.on_press(PanelButton::Trk, t0);
        assert_eq!(held.armed, Some(Hold::Trk));
        held.on_press(PanelButton::Trig1, t0);
        assert_eq!(
            held.armed, None,
            "a trig chord is one-shot: it disarms the prefix"
        );
    }

    /// TK2.1 C6 (D11): §0 A9 is reversed — a genuine re-tap of the same
    /// armed prefix (well outside the auto-repeat guard window) now
    /// disarms it, since without kitty release events "press it again" is
    /// the only cancel gesture available.
    #[test]
    fn sticky_prefix_retap_disarms() {
        let mut held = HeldState::new(false);
        let t0 = std::time::Instant::now();
        held.on_press(PanelButton::Trk, t0);
        assert_eq!(held.armed, Some(Hold::Trk));
        let t1 = t0 + std::time::Duration::from_millis(REPEAT_GUARD_MS as u64 + 50);
        held.on_press(PanelButton::Trk, t1);
        assert_eq!(
            held.armed, None,
            "D11: a deliberate re-tap outside the guard window disarms"
        );
    }

    /// TK2.1 C6 (D11): a same-prefix press clustered inside the guard
    /// window is OS auto-repeat, not a deliberate second tap — ignored,
    /// stays armed.
    #[test]
    fn sticky_prefix_autorepeat_within_guard_does_not_disarm() {
        let mut held = HeldState::new(false);
        let t0 = std::time::Instant::now();
        held.on_press(PanelButton::Trk, t0);
        let t1 = t0 + std::time::Duration::from_millis(REPEAT_GUARD_MS as u64 - 50);
        held.on_press(PanelButton::Trk, t1);
        assert_eq!(
            held.armed,
            Some(Hold::Trk),
            "D11: a press inside the guard window is auto-repeat, must stay armed"
        );
    }

    #[test]
    fn sticky_prefix_esc_still_disarms() {
        let mut held = HeldState::new(false);
        held.on_press(PanelButton::Ptn, std::time::Instant::now());
        held.on_esc();
        assert_eq!(held.armed, None);
    }

    #[test]
    fn nontrig_key_disarms_and_processes() {
        let mut held = HeldState::new(false);
        let t0 = std::time::Instant::now();
        held.on_press(PanelButton::Trk, t0);
        let consumed = held.on_press(PanelButton::Play, t0);
        assert_eq!(held.armed, None, "a non-trig, non-prefix key disarms");
        assert!(!consumed, "and is still processed normally (not swallowed)");
    }

    /// TK2 C3 (D6, kitty branch): unlike the sticky fallback, a kitty
    /// terminal delivers real release events — TRK stays armed exactly as
    /// long as it is physically held.
    #[test]
    fn kitty_press_arms_and_release_disarms() {
        let mut held = HeldState::new(true);
        assert!(held.on_kitty_press(PanelButton::Ptn));
        assert_eq!(held.armed, Some(Hold::Ptn));
        held.on_kitty_release(PanelButton::Ptn);
        assert_eq!(held.armed, None, "release must disarm the held prefix");
    }

    #[test]
    fn kitty_release_of_unrelated_button_is_a_noop() {
        let mut held = HeldState::new(true);
        held.on_kitty_press(PanelButton::Trk);
        held.on_kitty_release(PanelButton::Trig1);
        assert_eq!(
            held.armed,
            Some(Hold::Trk),
            "releasing a key that was never tracked as pressed must not disarm"
        );
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

    // ── TK2.1 C5a: ENC mode (D9) ────────────────────────────────────────

    #[test]
    fn enc_toggle_switches_trig_rows() {
        let held = HeldState::new(false);
        let mut screen = default_grid();
        screen.enc = false;
        let off = button_to_action(&held, &screen, PanelButton::Trig1, Mods::default());
        assert!(
            matches!(off, Action::ToggleStep { col: 0 }),
            "enc off: bare trig is a pad/step per D5/D6, got {off:?}"
        );

        screen.enc = true;
        let on = button_to_action(&held, &screen, PanelButton::Trig1, Mods::default());
        assert!(
            matches!(
                on,
                Action::EncoderJog {
                    col: 0,
                    dir: Dir::Next,
                    mag: Mag::Normal
                }
            ),
            "enc on: bare trig is an encoder jog, got {on:?}"
        );
    }

    #[test]
    fn enc_mode_works_on_grid_screen_not_only_param() {
        let held = HeldState::new(false);
        let screen = ScreenState {
            screen: Screen::Grid,
            rec: RecMode::Off,
            enc: true,
            lock_target_step: None,
        };
        let action = button_to_action(&held, &screen, PanelButton::Trig3, Mods::default());
        assert!(
            matches!(action, Action::EncoderJog { col: 2, .. }),
            "ENC must reach encoders on the Grid screen too, not only \
             Param; got {action:?}"
        );
    }

    #[test]
    fn param_screen_with_enc_off_still_pads() {
        let held = HeldState::new(false);
        let screen = ScreenState {
            screen: Screen::Param(0),
            rec: RecMode::Off,
            enc: false,
            lock_target_step: None,
        };
        let action = button_to_action(&held, &screen, PanelButton::Trig1, Mods::default());
        assert!(
            matches!(action, Action::LiveTrig { col: 0 }),
            "D6 invariant: with ENC off, a bare trig on Param is still a \
             pad, got {action:?}"
        );
    }

    /// §0 A10 regression: TRK armed still wins over ENC mode.
    #[test]
    fn enc_bare_trig_does_not_jog_while_trk_armed() {
        let held = HeldState {
            kitty: false,
            armed: Some(Hold::Trk),
            pressed: HashSet::new(),
            last_prefix_press: None,
        };
        let screen = ScreenState {
            screen: Screen::Grid,
            rec: RecMode::Off,
            enc: true,
            lock_target_step: None,
        };
        let action = button_to_action(&held, &screen, PanelButton::Trig5, Mods::default());
        assert!(
            matches!(action, Action::SelectTrack(4)),
            "an armed TRK prefix must win over ENC mode, got {action:?}"
        );
    }

    #[test]
    fn enc_func_is_coarse_ctrl_is_fine() {
        let held = HeldState::new(false);
        let screen = ScreenState {
            screen: Screen::Grid,
            rec: RecMode::Off,
            enc: true,
            lock_target_step: None,
        };
        let coarse = button_to_action(
            &held,
            &screen,
            PanelButton::Trig1,
            Mods { func: true, ctrl: false },
        );
        assert!(matches!(coarse, Action::EncoderJog { mag: Mag::Coarse, .. }));

        let fine = button_to_action(
            &held,
            &screen,
            PanelButton::Trig1,
            Mods { func: false, ctrl: true },
        );
        assert!(matches!(fine, Action::EncoderJog { mag: Mag::Fine, .. }));

        let normal = button_to_action(&held, &screen, PanelButton::Trig1, Mods::default());
        assert!(matches!(normal, Action::EncoderJog { mag: Mag::Normal, .. }));
    }

    /// Outside ENC mode, D8's original magnitude mapping stands: FUNC
    /// alone is Normal, FUNC+Ctrl is Fine — `Mag::Coarse` is unreachable
    /// without ENC.
    #[test]
    fn off_enc_fine_is_func_ctrl() {
        let held = HeldState::new(false);
        let screen = default_grid(); // enc: false
        let normal = button_to_action(
            &held,
            &screen,
            PanelButton::Trig1,
            Mods { func: true, ctrl: false },
        );
        assert!(matches!(normal, Action::EncoderJog { mag: Mag::Normal, .. }));

        let fine = button_to_action(
            &held,
            &screen,
            PanelButton::Trig1,
            Mods { func: true, ctrl: true },
        );
        assert!(matches!(fine, Action::EncoderJog { mag: Mag::Fine, .. }));
    }

    // ── TK2.1 C5b: the lock target (D15) ─────────────────────────────────

    #[test]
    fn lock_key_then_trig_sets_lock_target() {
        let held = HeldState {
            kitty: false,
            armed: Some(Hold::Lock),
            pressed: HashSet::new(),
            last_prefix_press: None,
        };
        let screen = default_grid(); // rec: Grid
        let action = button_to_action(&held, &screen, PanelButton::Trig5, Mods::default());
        assert!(matches!(action, Action::SetLockTarget(4)));
    }

    /// Lock-armed + a trig outside Grid mode consumes the arm (matching
    /// Trk/Ptn's one-shot precedent) but has nothing to target.
    #[test]
    fn lock_armed_trig_outside_grid_is_noop() {
        let held = HeldState {
            kitty: false,
            armed: Some(Hold::Lock),
            pressed: HashSet::new(),
            last_prefix_press: None,
        };
        let screen = ScreenState {
            screen: Screen::Grid,
            rec: RecMode::Off,
            enc: false,
            lock_target_step: None,
        };
        let action = button_to_action(&held, &screen, PanelButton::Trig5, Mods::default());
        assert!(matches!(action, Action::Noop));
    }

    /// Re-pressing the trig that set the current target clears it.
    #[test]
    fn retapping_the_locked_step_clears_it() {
        let held = HeldState::new(false);
        let screen = ScreenState {
            screen: Screen::Grid,
            rec: RecMode::Grid,
            enc: false,
            lock_target_step: Some(4),
        };
        let action = button_to_action(&held, &screen, PanelButton::Trig5, Mods::default());
        assert!(matches!(action, Action::ClearLockTarget));
    }

    /// TK2.1 C1 (D5): renamed from `rec_toggles_grid_recording` —
    /// `button_to_action` itself is state-independent (the actual
    /// Off/Grid/Live transition needs kitty + transport state, resolved in
    /// `lib.rs`'s dispatch); this pins that a bare REC press always
    /// resolves to `Action::ToggleRec`.
    #[test]
    fn rec_toggles_off_and_grid() {
        let held = HeldState::new(false);
        let action = button_to_action(&held, &default_grid(), PanelButton::Rec, Mods::default());
        assert!(matches!(action, Action::ToggleRec));
    }

    /// TK2.1 C1 (D5/D6): renamed from `trig_with_grid_rec_off_is_live_trig`
    /// — pad modes (`Off`/`Live`) resolve a trig to `LiveTrig`, carrying
    /// the column so `lib.rs` can select *and* sound track N (D6).
    #[test]
    fn pad_mode_trig_resolves_to_live_trig_with_column() {
        let held = HeldState::new(false);
        let screen = ScreenState {
            screen: Screen::Grid,
            rec: RecMode::Off,
            enc: false,
            lock_target_step: None,
        };
        let action = button_to_action(&held, &screen, PanelButton::Trig3, Mods::default());
        assert!(matches!(action, Action::LiveTrig { col: 2 }));

        let live_screen = ScreenState {
            screen: Screen::Grid,
            rec: RecMode::Live,
            enc: false,
            lock_target_step: None,
        };
        let action = button_to_action(&held, &live_screen, PanelButton::Trig3, Mods::default());
        assert!(
            matches!(action, Action::LiveTrig { col: 2 }),
            "Live is a pad mode too — trigs still select and sound"
        );
    }

    /// TK2.1 C1 (D5): the Grid-mode counterpart of the pad-mode test above
    /// — trigs write/clear steps instead.
    #[test]
    fn grid_mode_trig_toggles_step() {
        let held = HeldState::new(false);
        let action = button_to_action(&held, &default_grid(), PanelButton::Trig3, Mods::default());
        assert!(matches!(action, Action::ToggleStep { col: 2 }));
    }

    /// TK2.1 C1 (D5): REC held (kitty) + bare PLAY is the Live-record
    /// chord.
    #[test]
    fn rec_held_plus_play_enters_live_rec() {
        let mut held = HeldState::new(true);
        assert!(!held.on_kitty_press(PanelButton::Rec));
        let action = button_to_action(&held, &default_grid(), PanelButton::Play, Mods::default());
        assert!(matches!(action, Action::EnterLiveRec));
    }

    /// TK2.1 C1 (§0 A10 regression): an armed TRK prefix still wins over
    /// pad-mode trig resolution — a trig must chord, not sound live.
    #[test]
    fn armed_trk_still_wins_over_pads() {
        let held = HeldState {
            kitty: false,
            armed: Some(Hold::Trk),
            pressed: HashSet::new(),
            last_prefix_press: None,
        };
        let screen = ScreenState {
            screen: Screen::Grid,
            rec: RecMode::Off,
            enc: false,
            lock_target_step: None,
        };
        let action = button_to_action(&held, &screen, PanelButton::Trig5, Mods::default());
        assert!(matches!(action, Action::SelectTrack(4)));
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

    // ── TK2 C8: key remapping (D11/D14) ───────────────────────────────────

    #[test]
    fn keymap_resolves_user_binding_over_default() {
        // Built-in §2 table: 'q' → Trig1. A user binding must shadow it.
        let mut keymap = Keymap::default();
        keymap.bindings.insert(
            KeyBinding {
                code: KeyCode::Char('q'),
            },
            PanelButton::Trig9,
        );
        assert_eq!(key_to_button(&keymap, key('q')), Some(PanelButton::Trig9));
    }

    #[test]
    fn keymap_falls_through_when_unbound() {
        // A non-empty keymap with no entry for 'w' must still fall through
        // to the built-in table, not return None.
        let mut keymap = Keymap::default();
        keymap.bindings.insert(
            KeyBinding {
                code: KeyCode::Char('q'),
            },
            PanelButton::Trig9,
        );
        assert_eq!(key_to_button(&keymap, key('w')), Some(PanelButton::Trig2));
    }

    #[test]
    fn keymap_roundtrips_yaml() {
        let mut keymap = Keymap::default();
        keymap.bindings.insert(
            KeyBinding {
                code: KeyCode::Char('q'),
            },
            PanelButton::Trig9,
        );
        keymap.bindings.insert(
            KeyBinding { code: KeyCode::Tab },
            PanelButton::Song,
        );
        keymap.bindings.insert(
            KeyBinding {
                code: KeyCode::F(5),
            },
            PanelButton::Enc,
        );
        let yaml = keymap.to_yaml().expect("serialize");
        let (restored, skipped) = Keymap::from_yaml(&yaml).expect("deserialize");
        assert_eq!(restored.bindings, keymap.bindings);
        assert!(skipped.is_empty(), "every entry here is well-formed");
    }

    #[test]
    fn local_file_overrides_global() {
        let global_yaml = "q: Trig2\nw: Trig3\n";
        let local_yaml = "q: Trig5\n";
        let (merged, skipped) = Keymap::merge_sources(Some(global_yaml), Some(local_yaml));
        assert!(skipped.is_empty(), "every entry here is well-formed");
        assert_eq!(
            merged.bindings.get(&KeyBinding {
                code: KeyCode::Char('q')
            }),
            Some(&PanelButton::Trig5),
            "local must win on collision"
        );
        assert_eq!(
            merged.bindings.get(&KeyBinding {
                code: KeyCode::Char('w')
            }),
            Some(&PanelButton::Trig3),
            "global-only entries must survive the merge"
        );
    }

    #[test]
    fn button_name_roundtrips_every_variant() {
        for &(name, button) in BUTTON_NAMES {
            assert_eq!(button_name(button), name);
            assert_eq!(button_from_name(name), Some(button));
            assert_eq!(button_from_name(&name.to_lowercase()), Some(button));
        }
    }

    // ── TK2.1 C2: key chips (D3) ──────────────────────────────────────────

    /// Drift guard, both directions: every `DEFAULT_BINDINGS` entry must
    /// round-trip through `key_to_button`, and every key `key_to_button`
    /// resolves via the built-in fallthrough must appear in the table.
    #[test]
    fn default_bindings_match_key_to_button() {
        let keymap = Keymap::default();
        for &(code, button, _) in DEFAULT_BINDINGS {
            assert_eq!(
                key_to_button(&keymap, KeyEvent::new(code, KeyModifiers::NONE)),
                Some(button),
                "{code:?} must resolve to {button:?} via key_to_button"
            );
        }
        // Reverse: every button reachable by a built-in key has exactly
        // one PREFERRED entry in the table (key_label depends on this).
        for &(_, button, preferred) in DEFAULT_BINDINGS {
            if preferred {
                let preferred_count = DEFAULT_BINDINGS
                    .iter()
                    .filter(|(_, b, p)| *b == button && *p)
                    .count();
                assert_eq!(
                    preferred_count, 1,
                    "{button:?} must have exactly one preferred default key"
                );
            }
        }
    }

    #[test]
    fn key_label_falls_back_to_preferred_default() {
        let keymap = Keymap::default();
        assert_eq!(
            key_label(&keymap, PanelButton::Trig1),
            Some("q".to_string())
        );
        assert_eq!(
            key_label(&keymap, PanelButton::Trk),
            Some("tab".to_string())
        );
    }

    /// `x` is `Play`'s preferred key — `Space` is a valid alias but must
    /// never be what the chip shows.
    #[test]
    fn play_chip_is_x_not_space() {
        let keymap = Keymap::default();
        assert_eq!(
            key_label(&keymap, PanelButton::Play),
            Some("x".to_string())
        );
    }

    #[test]
    fn key_label_prefers_user_binding() {
        let mut keymap = Keymap::default();
        keymap.bindings.insert(
            KeyBinding {
                code: KeyCode::Char('j'),
            },
            PanelButton::Play,
        );
        assert_eq!(
            key_label(&keymap, PanelButton::Play),
            Some("j".to_string()),
            "a user binding must win over the default 'x'"
        );
    }

    /// D3 shadow-awareness: binding `q` away from `Trig1` must remove
    /// `Trig1`'s chip entirely, not leave the stale default lying around —
    /// `key_to_button` would resolve `q` to the new target, not `Trig1`.
    #[test]
    fn key_label_skips_default_key_shadowed_by_user_binding() {
        let mut keymap = Keymap::default();
        keymap
            .bindings
            .insert(KeyBinding { code: KeyCode::Char('q') }, PanelButton::Play);
        assert_eq!(
            key_label(&keymap, PanelButton::Trig1),
            None,
            "Trig1's default key 'q' is shadowed by a user binding to Play"
        );
    }

    #[test]
    fn key_name_roundtrips_named_tokens() {
        for code in [
            KeyCode::Tab,
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Up,
            KeyCode::F(12),
            KeyCode::Char(' '),
            KeyCode::Char('q'),
        ] {
            let name = key_name(code);
            assert_eq!(key_from_name(&name), Some(code), "round-trip of {name:?}");
        }
    }

    #[test]
    fn colon_is_unbindable() {
        assert!(is_unbindable(KeyCode::Char(':')));
        assert!(!is_unbindable(KeyCode::Char('c')));
    }

    /// D14's guarantee must hold for a hand-edited/tampered `keymap.yaml`
    /// too, not just the `:bind` verb parser — a file that smuggles a `:`
    /// entry must fail to load rather than silently gaining a dead-but-
    /// listed binding (post-C8 hostile review finding).
    #[test]
    fn from_yaml_rejects_unbindable_key() {
        let err = Keymap::from_yaml("':': Trig1\n").unwrap_err();
        assert!(
            err.contains("reserved"),
            "must reject a colon binding, got: {err}"
        );
    }

    /// TK2.1 C6 (D14): a stale entry naming a retired button (`Mute`,
    /// gone this same commit) must not reject the whole file — it's
    /// skipped, the rest of the file loads, and the skip is reported.
    #[test]
    fn keymap_with_retired_button_name_skips_only_that_binding() {
        let yaml = "m: Mute\nq: Trig9\n";
        let (keymap, skipped) = Keymap::from_yaml(yaml).expect("must not reject the whole file");
        assert_eq!(
            keymap.bindings.get(&KeyBinding { code: KeyCode::Char('q') }),
            Some(&PanelButton::Trig9),
            "the well-formed entry must still load"
        );
        assert!(
            !keymap.bindings.contains_key(&KeyBinding { code: KeyCode::Char('m') }),
            "the retired-button entry must not load"
        );
        assert_eq!(
            skipped,
            vec!["m: Mute".to_string()],
            "the skipped entry must be reported back to the caller"
        );
    }

    /// TK2.1 C6 (D12): `Mute` is no longer a valid `:bind`/`Keymap`
    /// button name (the screen/button were retired this commit) — must be
    /// rejected the same way any other unknown button name is.
    #[test]
    fn mute_button_name_is_rejected_by_bind() {
        assert_eq!(button_from_name("Mute"), None);
        assert_eq!(button_from_name("mute"), None);
    }
}
