use std::collections::HashSet;

use crossterm::event::KeyCode;
use crossterm::{cursor, execute, style, terminal, QueueableCommand};

/// What a key press targets, independent of emulator state (`active_row`).
/// Pure mapping output — unit-testable without crossterm or emulator state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeyTarget {
    /// Select the active track row (0–7). Cursor only; emits no hardware event.
    RowSelect(u8),
    /// Toggle a step pad (column 0–7) in the active row.
    Step(u8),
    /// Scene button 0–7 → control id 64+n.
    Scene(u8),
    /// Top control-row button 0–7 → control id 72+n.
    Control(u8),
}

/// Map a crossterm `KeyCode` to a `KeyTarget` (pure; case-insensitive).
///
/// - `1`–`8`            → `RowSelect`
/// - `Q W E R T Y U I`  → `Step` (cols 0–7)
/// - `A S D F G H J K`  → `Scene` (0–7)
/// - `Z X C V B N M ,`  → `Control` (0–7)
pub(super) fn key_to_target(code: KeyCode) -> Option<KeyTarget> {
    let c = match code {
        KeyCode::Char(c) => c.to_ascii_lowercase(),
        _ => return None,
    };
    match c {
        '1'..='8' => Some(KeyTarget::RowSelect(c as u8 - b'1')),

        'q' => Some(KeyTarget::Step(0)),
        'w' => Some(KeyTarget::Step(1)),
        'e' => Some(KeyTarget::Step(2)),
        'r' => Some(KeyTarget::Step(3)),
        't' => Some(KeyTarget::Step(4)),
        'y' => Some(KeyTarget::Step(5)),
        'u' => Some(KeyTarget::Step(6)),
        'i' => Some(KeyTarget::Step(7)),

        'a' => Some(KeyTarget::Scene(0)),
        's' => Some(KeyTarget::Scene(1)),
        'd' => Some(KeyTarget::Scene(2)),
        'f' => Some(KeyTarget::Scene(3)),
        'g' => Some(KeyTarget::Scene(4)),
        'h' => Some(KeyTarget::Scene(5)),
        'j' => Some(KeyTarget::Scene(6)),
        'k' => Some(KeyTarget::Scene(7)),

        'z' => Some(KeyTarget::Control(0)),
        'x' => Some(KeyTarget::Control(1)),
        'c' => Some(KeyTarget::Control(2)),
        'v' => Some(KeyTarget::Control(3)),
        'b' => Some(KeyTarget::Control(4)),
        'n' => Some(KeyTarget::Control(5)),
        'm' => Some(KeyTarget::Control(6)),
        ',' => Some(KeyTarget::Control(7)),

        _ => None,
    }
}

/// Control-id helpers (single source of truth, shared with `mod.rs`).
pub(super) const SCENE_BASE: u32 = 64;
pub(super) const CONTROL_BASE: u32 = 72;

/// Draw the full Launchpad surface to stdout: control row, 8×8 grid with the
/// active row marked, and the scene column. `pressed` holds currently-held
/// control ids (grid 0–63, scene 64–71, control 72–79).
pub(super) fn render(active_row: u8, pressed: &HashSet<u32>, mode_label: &str) {
    let mut out = std::io::stdout();
    let _ = execute!(out, cursor::SavePosition, cursor::MoveTo(0, 0));

    let cell = |id: u32| if pressed.contains(&id) { "[#]" } else { "[ ]" };

    let header = format!("Launchpad Emulator  [{mode_label}]  active row: {active_row}");
    let _ = out.queue(style::Print(format!("{header:<52}\r\n")));
    let _ = out.queue(style::Print(
        "  1-8 row · QWERTYUI steps · ASDFGHJK scene · ZXCVBNM, ctrl · Esc quit  \r\n",
    ));

    // Top control row (ids 72–79).
    let _ = out.queue(style::Print("   ctrl  "));
    for n in 0u32..8 {
        let _ = out.queue(style::Print(cell(CONTROL_BASE + n)));
    }
    let _ = out.queue(style::Print("\r\n"));

    let _ = out.queue(style::Print("        ┌────────────────────────┐\r\n"));
    for row in 0u32..8 {
        let marker = if row as u8 == active_row { '>' } else { ' ' };
        let _ = out.queue(style::Print(format!("    {marker}{row}  │")));
        for col in 0u32..8 {
            let _ = out.queue(style::Print(cell(row * 8 + col)));
        }
        // Scene button for this row (id 64+row).
        let _ = out.queue(style::Print(format!("│ {}\r\n", cell(SCENE_BASE + row))));
    }
    let _ = out.queue(style::Print("        └────────────────────────┘\r\n"));
    let _ = out.queue(terminal::Clear(terminal::ClearType::CurrentLine));

    let _ = execute!(out, cursor::RestorePosition);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_to_target_maps_number_row_to_row_select() {
        assert_eq!(
            key_to_target(KeyCode::Char('1')),
            Some(KeyTarget::RowSelect(0))
        );
        assert_eq!(
            key_to_target(KeyCode::Char('8')),
            Some(KeyTarget::RowSelect(7))
        );
    }

    #[test]
    fn key_to_target_maps_qwerty_to_steps() {
        assert_eq!(key_to_target(KeyCode::Char('q')), Some(KeyTarget::Step(0)));
        assert_eq!(key_to_target(KeyCode::Char('i')), Some(KeyTarget::Step(7)));
        // Case-insensitive.
        assert_eq!(key_to_target(KeyCode::Char('Q')), Some(KeyTarget::Step(0)));
    }

    #[test]
    fn key_to_target_maps_home_row_to_scene() {
        assert_eq!(key_to_target(KeyCode::Char('a')), Some(KeyTarget::Scene(0)));
        assert_eq!(key_to_target(KeyCode::Char('k')), Some(KeyTarget::Scene(7)));
    }

    #[test]
    fn key_to_target_maps_bottom_row_to_control() {
        assert_eq!(
            key_to_target(KeyCode::Char('z')),
            Some(KeyTarget::Control(0))
        );
        assert_eq!(
            key_to_target(KeyCode::Char(',')),
            Some(KeyTarget::Control(7))
        );
    }

    #[test]
    fn unmapped_key_returns_none() {
        assert_eq!(key_to_target(KeyCode::Char('9')), None);
        assert_eq!(key_to_target(KeyCode::Char('0')), None);
        assert_eq!(key_to_target(KeyCode::Esc), None);
        assert_eq!(key_to_target(KeyCode::Tab), None);
    }
}
