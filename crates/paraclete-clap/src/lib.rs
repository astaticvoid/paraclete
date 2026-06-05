// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete CLAP plugin adapter.
//!
//! Infrastructure shared by all plugin binaries. Plugin binaries are defined
//! in Commits 5 and 6 (`single_node.rs`, `subgraph.rs`). See ADR-024.

pub mod bridge;
pub mod ffi;
pub mod transport;

pub use single_node::SingleNodePlugin;
pub use subgraph::SubgraphPlugin;

pub(crate) mod plugin;
pub(crate) mod process_input;
pub(crate) mod single_node;
pub(crate) mod subgraph;
