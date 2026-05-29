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

pub use configurator::NodeConfigurator;
pub use executor::NodeExecutor;
pub use graph::NodeId;
pub use state_bus::StateBusSubscription;
pub use message::ConfigMessage;
