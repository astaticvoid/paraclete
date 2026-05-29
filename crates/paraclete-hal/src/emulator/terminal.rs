use std::collections::HashSet;

use crossterm::{cursor, execute, style, terminal, QueueableCommand};

/// Draw the Launchpad grid to stdout.
///
/// Called from `process()` when the render debounce has elapsed.
/// Pads in `pressed` are shown as `[#]`; others as `[ ]`.
///
/// Row 0 maps to keyboard Q–I, row 1 to A–K.
pub(super) fn render(pressed: &HashSet<u32>) {
    let mut out = std::io::stdout();

    // Save cursor position, move to render origin.
    let _ = execute!(out, cursor::SavePosition, cursor::MoveTo(0, 0));

    let header = "Launchpad Emulator   keys: Q W E R T Y U I";
    let _ = out.queue(style::Print(format!("{:<50}\r\n", header)));

    let _ = out.queue(style::Print("┌────────────────────────────────────┐\r\n"));

    for row in 0u32..8 {
        let _ = out.queue(style::Print("│ "));
        for col in 0u32..8 {
            let id = row * 8 + col;
            let cell = if pressed.contains(&id) { "[#]" } else { "[ ]" };
            let _ = out.queue(style::Print(cell));
        }
        // Scene button
        let scene_id = 64 + row;
        let scene = if pressed.contains(&scene_id) { "│[#]│" } else { "│[ ]│" };
        let _ = out.queue(style::Print(format!(" {scene}\r\n")));
    }

    let _ = out.queue(style::Print("└────────────────────────────────────┘\r\n"));
    let _ = out.queue(terminal::Clear(terminal::ClearType::CurrentLine));

    let _ = execute!(out, cursor::RestorePosition);
}

/// Map a crossterm `KeyCode` to a Launchpad pad id (row-major).
/// Returns `None` for keys not in the layout.
pub(super) fn key_to_pad(code: crossterm::event::KeyCode) -> Option<u32> {
    use crossterm::event::KeyCode::*;
    // Row 0: Q W E R T Y U I → pads 0–7
    // Row 1: A S D F G H J K → pads 8–15
    // Row 2: Z X C V B N M , → pads 16–23
    match code {
        Char('q') | Char('Q') => Some(0),
        Char('w') | Char('W') => Some(1),
        Char('e') | Char('E') => Some(2),
        Char('r') | Char('R') => Some(3),
        Char('t') | Char('T') => Some(4),
        Char('y') | Char('Y') => Some(5),
        Char('u') | Char('U') => Some(6),
        Char('i') | Char('I') => Some(7),

        Char('a') | Char('A') => Some(8),
        Char('s') | Char('S') => Some(9),
        Char('d') | Char('D') => Some(10),
        Char('f') | Char('F') => Some(11),
        Char('g') | Char('G') => Some(12),
        Char('h') | Char('H') => Some(13),
        Char('j') | Char('J') => Some(14),
        Char('k') | Char('K') => Some(15),

        Char('z') | Char('Z') => Some(16),
        Char('x') | Char('X') => Some(17),
        Char('c') | Char('C') => Some(18),
        Char('v') | Char('V') => Some(19),
        Char('b') | Char('B') => Some(20),
        Char('n') | Char('N') => Some(21),
        Char('m') | Char('M') => Some(22),
        Char(',')             => Some(23),
        _ => None,
    }
}
