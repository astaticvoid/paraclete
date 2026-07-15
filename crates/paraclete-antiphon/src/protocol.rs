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
    Hello {
        token: String,
        client: String,
    },
    PadDown {
        id: u32,
        vel: u16,
    },
    PadUp {
        id: u32,
    },
    Ping {
        ts: f64,
    },
    /// [W1+] continuous per-pad pressure.
    PadPres {
        id: u32,
        v: u16,
    },
    /// [W1] encoder ids 90–97; delta = accumulated detents this frame.
    Enc {
        id: u32,
        delta: i32,
    },
    /// [W1]
    EncPush {
        id: u32,
        pressed: bool,
    },
    /// [W1] semantic plane.
    SetParam {
        node: u32,
        param: String,
        v: f64,
    },
    /// [W1]
    BumpParam {
        node: u32,
        param: String,
        delta: f64,
    },
    /// [W1] declared node commands.
    NodeCmd {
        node: u32,
        cmd: u32,
        a0: i64,
        a1: f64,
    },
    /// [W2] request page layout for a track.
    GetViewMeta {
        track_id: u32,
        nonce: Option<String>,
    },
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
    Led {
        updates: Vec<LedMsg>,
    },
    Pong {
        ts: f64,
    },
    /// Sent before the server closes the connection.
    Bye {
        reason: String,
    },
    /// [W1] coalesced state mirror, ≤ ~30 Hz.
    State {
        updates: Vec<StateUpdate>,
    },
    /// [W1]
    Context {
        slots: Vec<ContextSlot>,
    },
    /// [W1] same shape as `welcome.nodes`, sent after apply_patch.
    Topology {
        nodes: Vec<NodeSummary>,
    },
    /// [W2] composite page layout for a track, in response to `get_view_meta`.
    ViewMeta {
        track_id: u32,
        nonce: Option<String>,
        engine_node_id: u32,
        engine_name: String,
        display_name: String,
        pages: Vec<ViewMetaPage>,
        chain: ViewMetaChain,
    },
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
    /// [W2] whether this node has a view (non-None `view` on the cap-doc).
    #[serde(default)]
    pub has_view: bool,
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

// ── W2: view_meta types ──────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewMetaPage {
    pub id: String,
    pub label: String,
    pub params: Vec<ViewMetaParam>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub envelopes: Vec<ViewMetaEnvelope>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub macros: Vec<ViewMetaMacro>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewMetaParam {
    pub id: String,
    pub node_id: u32,
    pub label: String,
    pub affordance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_group: Option<u32>,
    pub slot: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stepped: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing: Option<ViewMetaRouting>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewMetaEnvelope {
    pub id: u32,
    #[serde(rename = "type")]
    pub env_type: String,
    pub label: String,
    pub param_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewMetaMacro {
    pub name: String,
    pub targets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewMetaRouting {
    pub dest: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewMetaChain {
    pub nodes: Vec<u32>,
    pub node_labels: Vec<(u32, String)>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub routing: Vec<ViewMetaChainRoute>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewMetaChainRoute {
    pub source: u32,
    pub dest: String,
    pub param_id: String,
    pub value: f64,
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
            ClientMsg::Hello {
                token: "abcd1234".into(),
                client: "theoria-web/0.1".into(),
            },
            ClientMsg::PadDown { id: 13, vel: 65535 },
            ClientMsg::PadUp { id: 13 },
            ClientMsg::Ping { ts: 123456.7 },
            ClientMsg::PadPres { id: 13, v: 41000 },
            ClientMsg::Enc { id: 90, delta: -3 },
            ClientMsg::EncPush {
                id: 90,
                pressed: true,
            },
            ClientMsg::SetParam {
                node: 20,
                param: "cutoff".into(),
                v: 0.5,
            },
            ClientMsg::BumpParam {
                node: 20,
                param: "cutoff".into(),
                delta: 0.01,
            },
            ClientMsg::NodeCmd {
                node: 10,
                cmd: 16,
                a0: 3,
                a1: 0.0,
            },
            ClientMsg::GetViewMeta {
                track_id: 0,
                nonce: Some("req-1".into()),
            },
        ];
        for m in &msgs {
            round_trip_client(m);
        }
        // Spot-check wire tags match the spec exactly.
        assert!(serde_json::to_string(&msgs[1])
            .unwrap()
            .contains(r#""t":"pad_down""#));
        assert!(serde_json::to_string(&msgs[4])
            .unwrap()
            .contains(r#""t":"pad_pres""#));
        assert!(serde_json::to_string(&msgs[6])
            .unwrap()
            .contains(r#""t":"enc_push""#));
        assert!(serde_json::to_string(&msgs[9])
            .unwrap()
            .contains(r#""t":"node_cmd""#));
    }

    #[test]
    fn protocol_round_trip_server_msgs() {
        let msgs = [
            ServerMsg::Welcome {
                protocol: PROTOCOL_VERSION,
                device_id: 106,
                nodes: vec![NodeSummary {
                    id: 20,
                    type_tag: "analog_engine:kick".into(),
                    name: "AnalogEngine".into(),
                    has_view: false,
                    params: vec![ParamSummary {
                        id: 123,
                        name: "cutoff".into(),
                        min: 0.0,
                        max: 1.0,
                        default: 0.5,
                    }],
                }],
                transport: TransportSummary {
                    playing: true,
                    bpm: 120.0,
                },
            },
            ServerMsg::Led {
                updates: vec![LedMsg {
                    id: 13,
                    rgb: [255, 64, 0],
                }],
            },
            ServerMsg::Pong { ts: 123456.7 },
            ServerMsg::Bye {
                reason: "bad token".into(),
            },
            ServerMsg::State {
                updates: vec![StateUpdate {
                    path: "/node/20/param/cutoff".into(),
                    v: 0.62,
                }],
            },
            ServerMsg::Context {
                slots: vec![ContextSlot {
                    enc: 90,
                    node: 20,
                    param: "cutoff".into(),
                }],
            },
            ServerMsg::Topology { nodes: vec![] },
            ServerMsg::ViewMeta {
                track_id: 0,
                nonce: Some("req-1".into()),
                engine_node_id: 20,
                engine_name: "AnalogEngine".into(),
                display_name: "Kick".into(),
                pages: vec![ViewMetaPage {
                    id: "SRC".into(),
                    label: "Source".into(),
                    params: vec![ViewMetaParam {
                        id: "decay".into(),
                        node_id: 20,
                        label: "Decay".into(),
                        affordance: "EnvelopeCurve".into(),
                        env_group: Some(0),
                        slot: 0,
                        stepped: None,
                        options: None,
                        routing: None,
                    }],
                    envelopes: vec![ViewMetaEnvelope {
                        id: 0,
                        env_type: "AD".into(),
                        label: "Amp Envelope".into(),
                        param_ids: vec!["decay".into()],
                    }],
                    macros: vec![],
                }],
                chain: ViewMetaChain {
                    nodes: vec![20, 40],
                    node_labels: vec![(20, "Engine".into()), (40, "Filter".into())],
                    routing: vec![],
                },
            },
        ];
        for m in &msgs {
            round_trip_server(m);
        }
        assert!(serde_json::to_string(&msgs[0])
            .unwrap()
            .contains(r#""t":"welcome""#));
        assert!(serde_json::to_string(&msgs[1])
            .unwrap()
            .contains(r#""t":"led""#));
    }

    #[test]
    fn protocol_unknown_tag_is_error_not_panic() {
        assert!(serde_json::from_str::<ClientMsg>(r#"{"t":"warp","id":1}"#).is_err());
        assert!(serde_json::from_str::<ServerMsg>(r#"{"t":"warp"}"#).is_err());
        assert!(serde_json::from_str::<ClientMsg>("not json at all").is_err());
        assert!(serde_json::from_str::<ClientMsg>(r#"{"id":1}"#).is_err());
    }
}
