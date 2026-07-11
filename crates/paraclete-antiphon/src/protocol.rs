// SPDX-License-Identifier: GPL-3.0-or-later
//! Protocol v0 wire types (`design/phases/w0-interfaces.md`).
//!
//! JSON text frames, internally tagged with `"t"`. Unknown tags and malformed
//! frames are logged and dropped by the caller — deserialization returns
//! `Err`, never panics. Variants marked [W1] are parsed at W0 but not acted
//! on (parse-and-drop), so the W4 protocol freeze cannot foreclose them.

use serde::{Deserialize, Serialize};

/// Protocol revision carried in `welcome`. 0 = unstable pre-freeze.
pub const PROTOCOL_VERSION: u32 = 0;

// ── Client → server ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ClientMsg {
    /// Must be the first frame of a session.
    Hello { token: String, client: String },
    /// Pad ids: 0–63 grid (row*8+col), 64–71 scene, 72–79 control row.
    PadDown { id: u32, vel: u16 },
    PadUp { id: u32 },
    /// `ts` is client-clock milliseconds, echoed verbatim in `pong`.
    Ping { ts: f64 },
    /// [W1+] continuous per-pad pressure (16-bit). Reserved; parse-and-drop
    /// at W0 so the protocol freeze cannot foreclose poly aftertouch.
    PadPres { id: u32, v: u16 },
    /// [W1] encoder ids 90–97; delta = accumulated detents this frame.
    Enc { id: u32, delta: i32 },
    /// [W1]
    EncPush { id: u32, pressed: bool },
    /// [W1] semantic plane.
    SetParam { node: u32, param: String, v: f64 },
    /// [W1]
    BumpParam { node: u32, param: String, delta: f64 },
    /// [W1] declared node commands; pass-through.
    NodeCmd { node: u32, cmd: u32, a0: i64, a1: f64 },
}

// ── Server → client ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ServerMsg {
    Welcome {
        protocol: u32,
        device_id: u32,
        nodes: Vec<NodeSummary>,
        transport: TransportSummary,
    },
    /// Batched LED updates; a full-surface batch follows `welcome`.
    Led { updates: Vec<LedMsg> },
    Pong { ts: f64 },
    /// Sent before the server closes the connection.
    Bye { reason: String },
    /// [W1] coalesced state mirror, ≤ ~30 Hz.
    State { updates: Vec<StateUpdate> },
    /// [W1]
    Context { slots: Vec<ContextSlot> },
    /// [W1] same shape as `welcome.nodes`, sent after apply_patch.
    Topology { nodes: Vec<NodeSummary> },
}

/// One node in the `welcome`/`topology` snapshot. The app assembles these
/// from the configurator's cap-doc cache; antiphon never talks to the
/// configurator directly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeSummary {
    pub id: u32,
    pub type_tag: String,
    pub name: String,
    pub params: Vec<ParamSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamSummary {
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransportSummary {
    pub playing: bool,
    pub bpm: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LedMsg {
    pub id: u32,
    pub rgb: [u8; 3],
}

/// [W1]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateUpdate {
    pub path: String,
    pub v: f64,
}

/// [W1] One resolved encoder→node/param mapping slot. `enc` is the encoder
/// SLOT INDEX 0–7 (the trailing integer of the profile's `encoder_{i}`
/// context key), not a surface control id — each client maps its own
/// encoder controls onto slot indexes (defined 2026-07-10; was flagged as a
/// "boring documented choice" pending C4 validation, which never ran).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextSlot {
    pub enc: u32,
    pub node: u32,
    pub param: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_client(msg: &ClientMsg) {
        let json = serde_json::to_string(msg).expect("serialize");
        let back: ClientMsg = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(*msg, back, "round-trip failed for {json}");
    }

    fn round_trip_server(msg: &ServerMsg) {
        let json = serde_json::to_string(msg).expect("serialize");
        let back: ServerMsg = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(*msg, back, "round-trip failed for {json}");
    }

    #[test]
    fn protocol_round_trip_client_msgs() {
        let msgs = [
            ClientMsg::Hello { token: "abcd1234".into(), client: "theoria-web/0.1".into() },
            ClientMsg::PadDown { id: 13, vel: 65535 },
            ClientMsg::PadUp { id: 13 },
            ClientMsg::Ping { ts: 123456.7 },
            ClientMsg::PadPres { id: 13, v: 41000 },
            ClientMsg::Enc { id: 90, delta: -3 },
            ClientMsg::EncPush { id: 90, pressed: true },
            ClientMsg::SetParam { node: 20, param: "cutoff".into(), v: 0.5 },
            ClientMsg::BumpParam { node: 20, param: "cutoff".into(), delta: 0.01 },
            ClientMsg::NodeCmd { node: 10, cmd: 16, a0: 3, a1: 0.0 },
        ];
        for m in &msgs {
            round_trip_client(m);
        }
        // Spot-check wire tags match the spec exactly.
        assert!(serde_json::to_string(&msgs[1]).unwrap().contains(r#""t":"pad_down""#));
        assert!(serde_json::to_string(&msgs[4]).unwrap().contains(r#""t":"pad_pres""#));
        assert!(serde_json::to_string(&msgs[6]).unwrap().contains(r#""t":"enc_push""#));
        assert!(serde_json::to_string(&msgs[9]).unwrap().contains(r#""t":"node_cmd""#));
    }

    #[test]
    fn protocol_round_trip_server_msgs() {
        let msgs = [
            ServerMsg::Welcome {
                protocol: PROTOCOL_VERSION,
                device_id: 106,
                nodes: vec![NodeSummary {
                    id: 20,
                    type_tag: "analog_kick".into(),
                    name: "AnalogEngine".into(),
                    params: vec![ParamSummary {
                        id: 123, name: "cutoff".into(), min: 0.0, max: 1.0, default: 0.5,
                    }],
                }],
                transport: TransportSummary { playing: true, bpm: 120.0 },
            },
            ServerMsg::Led { updates: vec![LedMsg { id: 13, rgb: [255, 64, 0] }] },
            ServerMsg::Pong { ts: 123456.7 },
            ServerMsg::Bye { reason: "bad token".into() },
            ServerMsg::State { updates: vec![StateUpdate { path: "/node/20/param/cutoff".into(), v: 0.62 }] },
            ServerMsg::Context { slots: vec![ContextSlot { enc: 90, node: 20, param: "cutoff".into() }] },
            ServerMsg::Topology { nodes: vec![] },
        ];
        for m in &msgs {
            round_trip_server(m);
        }
        assert!(serde_json::to_string(&msgs[0]).unwrap().contains(r#""t":"welcome""#));
        assert!(serde_json::to_string(&msgs[1]).unwrap().contains(r#""t":"led""#));
    }

    #[test]
    fn protocol_unknown_tag_is_error_not_panic() {
        assert!(serde_json::from_str::<ClientMsg>(r#"{"t":"warp","id":1}"#).is_err());
        assert!(serde_json::from_str::<ServerMsg>(r#"{"t":"warp"}"#).is_err());
        assert!(serde_json::from_str::<ClientMsg>("not json at all").is_err());
        assert!(serde_json::from_str::<ClientMsg>(r#"{"id":1}"#).is_err());
    }
}
