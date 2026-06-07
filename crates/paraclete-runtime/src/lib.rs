// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete L1 Runtime — the nervous system.
//!
//! Owns graph topology, clock federation, node lifecycle, and the
//! configurator/executor split. Contains no DSP logic.

pub mod configurator;
pub mod executor;
pub mod graph;
pub mod state_bus;
pub mod message;
pub(crate) mod ring_buffer;

pub use configurator::{ConnectError, NodeConfigurator, NodeOrDevice};
pub use executor::NodeExecutor;
pub use graph::NodeId;
pub use message::ConfigMessage;

// Re-export StateBusSubscription from L2 for convenience.
pub use paraclete_node_api::StateBusSubscription;
