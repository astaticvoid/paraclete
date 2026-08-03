// SPDX-License-Identifier: LGPL-3.0-or-later

/// A command directed at a specific graph node.
///
/// Fixed-size (24 bytes) — safe to queue in a lock-free ring buffer without
/// allocation. Node-specific type IDs start at 16; 0–15 are reserved for
/// universal commands defined here and in future platform releases.
#[derive(Clone, Copy, Debug)]
pub struct NodeCommand {
    pub target_id: u32,
    /// 0 = CMD_SET_PARAM, 1 = CMD_BUMP_PARAM, 16+ = node-specific.
    pub type_id: u32,
    /// Integer argument — typically the param_id or step index.
    pub arg0: i64,
    /// Float argument — typically the parameter value or delta.
    pub arg1: f64,
}

/// Universal: set a declared parameter to an absolute value.
/// arg0 = param_id (u32 cast), arg1 = value (clamped to declared range).
pub const CMD_SET_PARAM: u32 = 0;

/// Universal: adjust a declared parameter by a signed delta.
/// arg0 = param_id (u32 cast), arg1 = delta (result clamped to declared range).
pub const CMD_BUMP_PARAM: u32 = 1;

/// Universal instrument command: live-trigger a voice (pad "trigger mode").
/// arg0 = note number (< 0 → the node's default/last note); arg1 = velocity
/// 0.0–1.0 (<= 0.0 → default 0.79). Same retrigger path as a `NoteOn` event.
/// Numerically this falls in the node-specific range (>= 16) but is
/// designated universal — implemented identically by every instrument node
/// (`AnalogEngine`, `FmEngine`, `Sampler`) rather than being node-type-specific.
pub const CMD_TRIGGER: u32 = 19;

/// P11 C3: snapshot the active pattern into the sequencer's pre-allocated
/// shadow slot (node-specific, ADR-039 reserves 39–45 for the P11 family).
pub const CMD_TEMP_SAVE: u8 = 39;

/// P11 C3: restore the shadow pattern into the active pattern.
pub const CMD_TEMP_RELOAD: u8 = 40;

/// P11 C4: set or toggle the active pattern's muted flag. `arg0`:
/// 0 = off, 1 = on, 2 = toggle.
pub const CMD_SET_PATTERN_MUTE: u8 = 41;

/// P11 C4: defer a global-mute change to the next pattern wrap. `arg0`:
/// 0 = off, 1 = on. The sequencer stores the pending value and applies it
/// at its own wrap (ADR-039 decision 6 — per-node, sample-deterministic).
pub const CMD_PREPARE_MUTE: u8 = 42;

/// P11 C4: defer a pattern-mute change for the active pattern to the next
/// pattern wrap. `arg0`: 0 = off, 1 = on. Applied at the wrap and cleared.
pub const CMD_PREPARE_PATTERN_MUTE: u8 = 43;

/// P11 C6 (OQ-T25 resolution, user decision 2026-08-02): arm or disarm
/// live erasing (Elektron-style — hold NO while the transport plays).
/// `arg0`: 0 = off, 1 = on. While armed and playing, each step the
/// playhead reaches is cleared as it passes (it does not sound). Cleared
/// on transport stop.
pub const CMD_LIVE_ERASE: u8 = 44;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_command_is_24_bytes() {
        // u32 + u32 + i64 + f64 = 4 + 4 + 8 + 8 = 24
        assert_eq!(std::mem::size_of::<NodeCommand>(), 24);
    }

    #[test]
    fn cmd_constants_are_in_universal_range() {
        assert!(CMD_SET_PARAM < 16);
        assert!(CMD_BUMP_PARAM < 16);
    }
}
