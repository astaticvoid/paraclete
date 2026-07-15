/// Re-exports of `midi2` crate types needed to build and decode UMP messages.
///
/// Third-party node authors who depend only on `paraclete-node-api` can access
/// all MIDI 2.0 types through this module without a direct `midi2` dependency.
///
/// The fixed buffer type `[u32; 4]` is used throughout — it covers the maximum
/// UMP packet size (128 bits) and is `Copy`, keeping events allocation-free.
pub use midi2::channel_voice2::{ChannelVoice2, NoteOff, NoteOn};
pub use midi2::ux::{u4, u7};
pub use midi2::{Channeled, Grouped};
