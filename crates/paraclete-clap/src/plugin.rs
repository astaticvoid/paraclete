// SPDX-License-Identifier: GPL-3.0-or-later
//! CLAP plugin entry point macro and factory boilerplate.
//! Concrete plugin binaries are in single_node.rs (Commit 5) and subgraph.rs (Commit 6).
//!
//! When clap-sys is added as a dependency (Commit 5), this module will expand
//! to the full `clap_plugin_entry!` macro and factory implementation.
