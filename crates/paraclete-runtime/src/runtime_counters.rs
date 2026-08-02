// SPDX-License-Identifier: GPL-3.0-or-later
//! Runtime counters — re-exported from `paraclete-node-api` (L2).
//!
//! The canonical definition lives at L2 so that `paraclete-hal` (L0) can
//! depend on `RuntimeCounters` without pulling in L1 (`paraclete-runtime`).
//! L1 re-exports it here for backward compatibility with internal callers.

pub use paraclete_node_api::RuntimeCounters;
