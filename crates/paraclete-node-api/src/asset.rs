// SPDX-License-Identifier: LGPL-3.0-or-later
//! Asset-delivery plane (AN0, ADR-040 decision 4).
//!
//! `NodeCommand` (`command.rs`) stays fixed-width and carries no heap data.
//! Sample payloads are too large for that plane, so they travel as
//! `Arc<SampleData>` on a dedicated SPSC ring (`paraclete-runtime`) instead —
//! moving/cloning an `Arc` is a pointer copy and an atomic refcount bump,
//! never an allocation, so it stays audio-thread-safe without needing
//! `NodeCommand`'s fixed-size discipline.

use std::sync::Arc;

/// Decoded, ready-to-play sample audio.
///
/// Mono — the sampler's existing single-channel buffer shape. Stereo
/// capture/playback is an inherited limitation, not introduced by AN0
/// (ADR-040 review note 5; `design/phases/an0-anamnesis.md` §0 D5).
#[derive(Debug)]
pub struct SampleData {
    pub samples: Vec<f32>,
    pub rate: f32,
    pub frames: usize,
}

/// Delivers a decoded sample to one node's asset cache, targeting a pool
/// slot. `node_id`-unicast, not a pool-wide broadcast type — the caller
/// decides which slots a given node needs and sends exactly those
/// (`an0-anamnesis.md` §0 D4).
#[derive(Clone, Debug)]
pub struct SwapSample {
    pub node_id: u32,
    pub pool_slot: u32,
    pub data: Arc<SampleData>,
}
