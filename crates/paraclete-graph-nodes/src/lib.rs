// SPDX-License-Identifier: GPL-3.0-or-later
//! `paraclete-graph-nodes` — nodes that contain an inner `NodeExecutor`.
//!
//! This is the only crate permitted to implement `Node` while also depending
//! on `paraclete-runtime`. `paraclete-nodes` cannot do this (ADR-022
//! portability rule). See ADR-023 for the instrument-encapsulation rationale.

pub mod inner_graph;
pub use inner_graph::InnerGraphNode;
