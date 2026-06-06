// SPDX-License-Identifier: GPL-3.0-or-later
pub mod project;
pub mod instrument;
pub mod builder;

/// Returns true when the TUI should be started (i.e. `--no-tui` was not passed).
pub fn tui_enabled(no_tui: bool) -> bool { !no_tui }
