/// Messages sent from `NodeConfigurator` (main thread) to `NodeExecutor`
/// (audio thread) via the lock-free ring buffer.
///
/// All variants must be `Send`. No allocations on the audio thread — messages
/// that carry heap data do so by transferring ownership (sender allocates,
/// receiver takes the pointer).
#[non_exhaustive]
pub enum ConfigMessage {
    /// Start / stop transport playback.
    SetPlaying(bool),

    /// Change the global BPM.
    SetBpm(f64),

    /// Update a parameter on a node.
    SetParam { node_id: u32, param_id: u32, value: f64 },
}
