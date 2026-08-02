// SPDX-License-Identifier: LGPL-3.0-or-later
//! Audio processor trait — the contract between L0 HAL and L1 Runtime.
//!
//! `paraclete-hal` depends on this trait (at L2) instead of the concrete
//! L1 `NodeExecutor` type, satisfying the five-layer boundary constraint.

use std::any::Any;
use std::sync::Arc;

use crate::runtime_counters::RuntimeCounters;

/// The audio-thread processing contract.
///
/// Implemented by `paraclete_runtime::NodeExecutor` (L1).
/// Consumed by `paraclete_hal::AudioBackend` (L0) — L0 depends on L2, not L1.
///
/// The `Any` supertrait methods enable L1-aware code (e.g. dynamic topology
/// patches in `paraclete-app`) to downcast back to the concrete `NodeExecutor`
/// when it needs access to L1-specific operations like `drain_nodes()`.
pub trait AudioProcessor: Send {
    /// Render one block of audio. Called from the cpal audio callback.
    ///
    /// `out_interleaved` is a pre-cleared buffer of `block_size * channels` samples.
    /// The implementation must sum its graph output into it; consumers pre-clear it.
    fn process(&mut self, out_interleaved: &mut [f32], channels: usize);

    /// The fixed rendering block size in frames (e.g. 512).
    fn block_size(&self) -> usize;

    /// Attach shared runtime counters for publishing engine metrics.
    fn set_counters(&mut self, counters: Arc<RuntimeCounters>);

    /// Read-only access to the shared runtime counters.
    fn counters(&self) -> &Arc<RuntimeCounters>;

    /// Enable downcasting to the concrete implementation type.
    /// Used by topology-patch code that needs L1-specific operations.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Enable downcasting from `Box<dyn AudioProcessor>` to the concrete type.
    /// Used by topology-patch code that needs to recover the `NodeExecutor`.
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}
