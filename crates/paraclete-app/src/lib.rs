// SPDX-License-Identifier: GPL-3.0-or-later
pub mod builder;
pub mod instrument;
pub mod patch;
pub mod project;
pub mod registry;

pub use patch::{apply_patch, PatchError, TopologyChange};
pub use registry::{build_registry, NodeRegistry};

/// Returns true when the TUI should be started (i.e. `--no-tui` was not passed).
pub fn tui_enabled(no_tui: bool) -> bool {
    !no_tui
}
