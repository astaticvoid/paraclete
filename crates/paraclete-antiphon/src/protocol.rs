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
    /// [P11 C7] app-op verbs. `kit_id` is a 0-based kit-slot index.
    KitLoad {
        kit_id: u8,
    },
    KitSave {
        name: String,
    },
    KitCommit {},
    KitReload {},
    TempSave {},
    TempReload {},
    SetPerformMode {
        on: bool,
    },
    /// `None` = unbind the slot.
    BindKit {
        slot: usize,
        kit_id: Option<u8>,
    },
    /// [P11 C7] kit-list query (not an app-op); the server replies with
    /// `ServerMsg::KitList`, read at query time.
    ListKits {},
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
        slots: Vec<ContextSlotLike>,
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
        /// The engine's Rule name — the machine it was **built with**, frozen
        /// (BUG-058 decision, 2026-08-02). Does NOT follow a machine switch.
        /// The active machine's name is `variants[i].name` where
        /// `variants[i].value == variants[i].active`.
        engine_name: String,
        display_name: String,
        /// The layout for the machine each host was **constructed** with, not
        /// the one it is on now. Antiphon assembles from the cap-doc snapshot
        /// taken at `add_node` and nothing re-runs it, so setting `machine`
        /// on a node and re-requesting `view_meta` returns the same `active`
        /// until restart (#157). A client that has watched
        /// `/node/{id}/param/machine` change should draw the matching entry
        /// of `variants` rather than this field.
        pages: Vec<ViewMetaPage>,
        chain: ViewMetaChain,
        /// [MM] ADR-041 machine variants. Absent for a track whose chain has
        /// no machine host, which is every track today that is not an
        /// `AnalogEngine` or `FmEngine`.
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        variants: Vec<ViewMetaVariantSet>,
    },
    /// [P11 C7] reply to `list_kits`: the `/context/kits` bus line
    /// (`idx:name;idx:name;...`, empty slots omitted), read at query time.
    KitList {
        kits: String,
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

/// P11 C7: a raw `/context/*` path value in the `context` snapshot — the
/// web KIT tab's source for `/context/kits`, `/context/kit_binding` and
/// `/context/perform` (the encoder slots above carry only encoder
/// bindings; these carry the perform-state text/number values). Untagged
/// so the wire value is the bare JSON scalar the client already types as
/// `string | number`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContextPathValue {
    Text(String),
    Num(f64),
}

/// One element of a `context` snapshot: either an encoder slot (legacy
/// shape) or a raw path slot (P11 C7). Untagged so existing clients keep
/// parsing the encoder slots unchanged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContextSlotLike {
    Encoder(ContextSlot),
    Path(ContextPathSlot),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextPathSlot {
    pub path: String,
    pub value: ContextPathValue,
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
    /// Present and `true` for an integer-stepped param; absent means
    /// continuous. Never sent as `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stepped: Option<bool>,
    /// Named values for a stepped param, **indexed by the param's value** —
    /// `options[v]` labels value `v`, and `null` is a value with no name.
    /// It is *not* an ordered list of choices: reading `options[i]` as the
    /// i-th choice is right only for a param whose values start at 0 and run
    /// contiguously, which is true of `machine` and need not be of the next
    /// stepped param to arrive.
    ///
    /// Absent when the param has no named values, and also when its values
    /// are too spread to index densely. For a machine selector the
    /// authoritative list is always `ViewMetaVariantSet::variants`, which
    /// carries `value` explicitly; this field is the by-value view of the
    /// same names and never disagrees with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<Option<String>>>,
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

/// [MM] Every machine one node in the track's chain can be (ADR-041).
///
/// A client switches machines by setting `select_param` on `node_id` and
/// drawing the matching entry's `pages`. It never re-merges: the pages here
/// are the whole track's, already aligned, because a client that assembled
/// them itself would have to re-implement 8-slot contributor alignment and
/// would drift from the server's.
///
/// **Each entry's `pages` assume every *other* machine host in the chain
/// stays on the machine `active` names for it.** With one host per chain —
/// every track that ships, since an engine has no audio input and so can
/// never be another track's chain node — that assumption is vacuous. With
/// two, switching host B and then drawing `A.variants[j].pages` renders B's
/// stale contribution, and because a contributor reserves whole sub-pages,
/// every slot after B's shifts. Nothing here records which selections the
/// pre-merge assumed; a client with two hosts in one chain must re-request
/// `view_meta` after a switch rather than trust the other host's entry.
///
/// **Size grows with the machine count.** Every entry carries the whole
/// track's pages, so the message is O(machines x chain length). Three
/// machines is the shipped case; a 16- or 32-machine bank would multiply
/// `view_meta` by that with no cap and no paging (#158).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewMetaVariantSet {
    pub node_id: u32,
    /// Param name to `set_param` on `node_id` to change machine.
    ///
    /// Absent when there is no such param to name — either the node declares
    /// variants without flagging an identity param, or it flags one its
    /// cap-doc does not declare. Nothing in tree does either. Absent means the
    /// machine cannot be changed; it is deliberately not a `param_{id}`
    /// placeholder, which would look live and write nowhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub select_param: Option<String>,
    /// The value `ViewMeta::pages` was built for.
    pub active: u32,
    pub variants: Vec<ViewMetaVariant>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewMetaVariant {
    /// `select_param`'s value that selects this machine.
    pub value: u32,
    pub name: String,
    pub pages: Vec<ViewMetaPage>,
    /// Display ranges for this machine. The node's own parameter bank holds
    /// the **union** across machines and is never narrowed, so a client that
    /// clamps to the bank's range would let a performer dial a value this
    /// machine does not use. Clamp input to these instead (ADR-041 §0 A1).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub overlays: Vec<ViewMetaOverlay>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewMetaOverlay {
    pub param: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// This param *is* the node's identity, not a setting: reject it as a
    /// p-lock target and as a scene-morph destination (ADR-041 §0 A4).
    #[serde(skip_serializing_if = "is_false", default)]
    pub identity: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
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
            ClientMsg::KitLoad { kit_id: 7 },
            ClientMsg::KitSave {
                name: "Kick Basic".into(),
            },
            ClientMsg::KitCommit {},
            ClientMsg::KitReload {},
            ClientMsg::TempSave {},
            ClientMsg::TempReload {},
            ClientMsg::SetPerformMode { on: true },
            ClientMsg::BindKit {
                slot: 2,
                kit_id: Some(7),
            },
            ClientMsg::BindKit {
                slot: 3,
                kit_id: None,
            },
            ClientMsg::ListKits {},
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
        // P11 C7 wire tags + payloads.
        for (raw, tag) in [
            (r#"{"t":"kit_load","kit_id":7}"#, "kit_load"),
            (r#"{"t":"kit_save","name":"Kick"}"#, "kit_save"),
            (r#"{"t":"kit_commit"}"#, "kit_commit"),
            (r#"{"t":"kit_reload"}"#, "kit_reload"),
            (r#"{"t":"temp_save"}"#, "temp_save"),
            (r#"{"t":"temp_reload"}"#, "temp_reload"),
            (r#"{"t":"set_perform_mode","on":true}"#, "set_perform_mode"),
            (r#"{"t":"bind_kit","slot":2,"kit_id":7}"#, "bind_kit"),
            (r#"{"t":"bind_kit","slot":3,"kit_id":null}"#, "bind_kit"),
            (r#"{"t":"list_kits"}"#, "list_kits"),
        ] {
            let msg: ClientMsg = serde_json::from_str(raw).expect("new variant must parse");
            let serialized = serde_json::to_string(&msg).expect("serialize");
            assert!(
                serialized.contains(&format!(r#""t":"{tag}""#)),
                "wire tag mismatch for {raw} -> {serialized}"
            );
            assert_eq!(
                serde_json::to_string(&msg).unwrap(),
                raw,
                "exact wire shape must match the spec for {raw}"
            );
        }
        assert!(
            matches!(
                serde_json::from_str::<ClientMsg>(r#"{"t":"bind_kit","slot":0}"#),
                Ok(ClientMsg::BindKit {
                    slot: 0,
                    kit_id: None
                })
            ),
            "missing kit_id deserializes as None (Option default)"
        );
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
                slots: vec![ContextSlotLike::Encoder(ContextSlot {
                    enc: 90,
                    node: 20,
                    param: "cutoff".into(),
                })],
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
                variants: vec![ViewMetaVariantSet {
                    node_id: 20,
                    select_param: Some("machine".into()),
                    active: 0,
                    variants: vec![ViewMetaVariant {
                        value: 0,
                        name: "AnalogKick".into(),
                        pages: vec![ViewMetaPage {
                            id: "SRC".into(),
                            label: "Source".into(),
                            params: vec![ViewMetaParam {
                                id: "machine".into(),
                                node_id: 20,
                                label: "Machine".into(),
                                affordance: "None".into(),
                                env_group: None,
                                slot: 0,
                                stepped: Some(true),
                                // Value 1 deliberately unnamed, so the
                                // round-trip covers a hole rather than only
                                // a dense list.
                                options: Some(vec![
                                    Some("AnalogKick".into()),
                                    None,
                                    Some("AnalogHiHat".into()),
                                ]),
                                routing: None,
                            }],
                            envelopes: vec![],
                            macros: vec![],
                        }],
                        overlays: vec![ViewMetaOverlay {
                            param: "machine".into(),
                            min: 0.0,
                            max: 2.0,
                            default: 0.0,
                            identity: true,
                        }],
                    }],
                }],
            },
            ServerMsg::KitList {
                kits: "0:Kick Basic;3:Snare".into(),
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
        // P11 C7: kit_list wire shape.
        let raw = r#"{"t":"kit_list","kits":"0:Kick Basic;3:Snare"}"#;
        let msg: ServerMsg = serde_json::from_str(raw).expect("kit_list must parse");
        assert_eq!(
            serde_json::to_string(&msg).unwrap(),
            raw,
            "exact wire shape must match the spec"
        );
        assert!(matches!(
            msg,
            ServerMsg::KitList { kits } if kits == "0:Kick Basic;3:Snare"
        ));
    }

    #[test]
    fn protocol_unknown_tag_is_error_not_panic() {
        assert!(serde_json::from_str::<ClientMsg>(r#"{"t":"warp","id":1}"#).is_err());
        assert!(serde_json::from_str::<ServerMsg>(r#"{"t":"warp"}"#).is_err());
        assert!(serde_json::from_str::<ClientMsg>("not json at all").is_err());
        assert!(serde_json::from_str::<ClientMsg>(r#"{"id":1}"#).is_err());
    }
}
