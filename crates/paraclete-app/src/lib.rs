// SPDX-License-Identifier: GPL-3.0-or-later
pub mod project;
pub mod instrument;
pub mod builder;
pub mod registry;
pub mod patch;

pub use registry::{NodeRegistry, build_registry};
pub use patch::{TopologyChange, PatchError, apply_patch};

/// Returns true when the TUI should be started (i.e. `--no-tui` was not passed).
pub fn tui_enabled(no_tui: bool) -> bool { !no_tui }
