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

/// AN1 capture-transfer plane (ADR-040 decision 2; `an1-anamnesis.md` §0
/// D8-D9). A `RecorderNode`'s fulfilled `CMD_RECORDER_CAPTURE`: the ring
/// contents it copied out, moving from the audio thread to the main
/// thread for `SamplePool` assembly.
///
/// Unlike `SwapSample`, this carries a plain owned `Box<[f32]>`, not an
/// `Arc` — a capture chunk has exactly one producer (the recorder that
/// made it) and exactly one consumer (whoever assembles it into a pool
/// entry), so ownership genuinely transfers each direction; nothing ever
/// needs to share it. `buf`'s capacity may exceed `frames` (it is a
/// reused fixed-capacity scratch buffer, not sized exactly to each
/// request) — only `buf[..frames]` is the actual captured audio.
#[derive(Debug)]
pub struct RecorderCapture {
    pub node_id: u32,
    pub request_id: u32,
    pub frames: usize,
    pub buf: Box<[f32]>,
}

/// The return leg of the capture-transfer plane: once the main thread has
/// finished reading `RecorderCapture::buf`, it sends the drained buffer
/// back so the originating `RecorderNode` can reuse it for a future
/// capture without an audio-thread allocation (D9).
#[derive(Debug)]
pub struct CaptureBufferReturn {
    pub node_id: u32,
    pub buf: Box<[f32]>,
}
