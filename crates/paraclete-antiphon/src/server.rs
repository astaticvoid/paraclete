// SPDX-License-Identifier: GPL-3.0-or-later
//! WebSocket server: accept loop, token handshake, per-client I/O threads.
//!
//! Threading (ADR-031: blocking I/O + rtrb, no tokio — the midir pattern):
//! one accept thread; one I/O thread per client. Each client thread owns its
//! WebSocket exclusively and alternates a short-timeout read with draining
//! its outbound `mpsc` — a single owner per socket means tungstenite's
//! auto-queued pong replies can never interleave with kerygma frames
//! mid-write. (The w0 spec sketched split read/write threads; the combined
//! loop is recorded in `w0-report.md` — same data flow, one less writer.)
//!
//! Handshake and frame-routing decisions are pure functions so the protocol
//! behaviour is unit-testable without sockets.

use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use paraclete_node_api::{NodeCommand, SurfaceEvent, CMD_BUMP_PARAM, CMD_SET_PARAM};
use tungstenite::{Message, WebSocket};

use crate::kerygma::{ClientTable, MAX_CLIENTS};
use crate::protocol::{ClientMsg, NodeSummary, ServerMsg, TransportSummary, PROTOCOL_VERSION};
use crate::surface::client_msg_to_surface_event;
use crate::view::ViewRegistry;

/// The first frame must be a valid `hello` within this window.
const HELLO_TIMEOUT: Duration = Duration::from_secs(3);
/// Read timeout for the steady-state I/O loop; bounds outbound-frame latency.
const IO_POLL: Duration = Duration::from_millis(15);
/// Per-message rate cap on `bump_param` deltas (w1-interfaces.md §Commit 3),
/// as a fraction of the resolved param's declared range. Distinct from the
/// node-side clamp — the `ParameterBank` clamps the *result* of applying the
/// delta; this only bounds how far one message can move it. Range-relative
/// because deltas are in param units: a fixed absolute cap silently froze
/// wide-range params like a 200–8000 Hz cutoff (found 2026-07-10).
const BUMP_DELTA_RANGE_FRAC: f64 = 0.5;

/// Immutable per-session data shared with every client thread.
pub struct SessionInfo {
    pub token: String,
    pub device_id: u32,
    pub nodes: Vec<NodeSummary>,
    pub transport: TransportSummary,
    /// [W2] per-track chain config + node rules for composite view assembly.
    pub view_registry: ViewRegistry,
}

/// Producer ends of the surface node's inbound rings, indexed by client slot.
/// A client thread takes its slot's producer at connect and returns it at
/// disconnect, so the ring survives across sessions on the same slot.
pub type ProducerPool = Arc<Mutex<Vec<Option<rtrb::Producer<SurfaceEvent>>>>>;

// ── Pure protocol decisions ───────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum HandshakeDecision {
    Accept { client: String },
    Reject { reason: &'static str },
}

/// Evaluate the first frame of a session against the expected token.
/// An empty expected token = open mode (the 2026-07-10 default): any client
/// token is accepted, including a stale code remembered from a previous
/// `--token` run — open must never bounce anyone.
pub fn evaluate_hello(frame: &str, expected_token: &str) -> HandshakeDecision {
    match serde_json::from_str::<ClientMsg>(frame) {
        Ok(ClientMsg::Hello { token, client }) => {
            if expected_token.is_empty() || token == expected_token {
                HandshakeDecision::Accept { client }
            } else {
                HandshakeDecision::Reject {
                    reason: "bad token",
                }
            }
        }
        Ok(_) => HandshakeDecision::Reject {
            reason: "hello must be first frame",
        },
        Err(_) => HandshakeDecision::Reject {
            reason: "malformed hello",
        },
    }
}

/// What the I/O loop should do with a post-handshake frame.
#[derive(Debug)]
pub enum FrameAction {
    /// Push this event into the client's slot ring.
    Emit(SurfaceEvent),
    /// Reply with `pong` echoing the timestamp.
    Pong(f64),
    /// A resolved semantic-plane command, ready to reach the graph.
    Command(NodeCommand),
    /// Log and drop (reserved messages, out-of-range ids, unresolvable
    /// semantic-plane references).
    Drop(&'static str),
    /// [W2] Reply with a `view_meta` message. The I/O thread serialises and
    /// sends it directly to the client that requested it.
    ViewMeta(ServerMsg),
}

/// Resolve a semantic-plane message against the cap-doc snapshot in
/// `session.nodes`. Pure; unknown node/param references drop rather than
/// erroring — a stale client (old topology) must not be able to crash or
/// wedge the server.
fn resolve_semantic(msg: ClientMsg, session: &SessionInfo) -> FrameAction {
    match msg {
        ClientMsg::SetParam { node, param, v } => {
            let Some(n) = session.nodes.iter().find(|n| n.id == node) else {
                return FrameAction::Drop("set_param: unknown node or param");
            };
            let Some(p) = n.params.iter().find(|p| p.name == param) else {
                return FrameAction::Drop("set_param: unknown node or param");
            };
            let clamped = v.clamp(p.min, p.max);
            FrameAction::Command(NodeCommand {
                target_id: node,
                type_id: CMD_SET_PARAM,
                arg0: p.id as i64,
                arg1: clamped,
            })
        }
        ClientMsg::BumpParam { node, param, delta } => {
            let Some(n) = session.nodes.iter().find(|n| n.id == node) else {
                return FrameAction::Drop("bump_param: unknown node or param");
            };
            let Some(p) = n.params.iter().find(|p| p.name == param) else {
                return FrameAction::Drop("bump_param: unknown node or param");
            };
            let cap = (p.max - p.min).abs() * BUMP_DELTA_RANGE_FRAC;
            let clamped = delta.clamp(-cap, cap);
            FrameAction::Command(NodeCommand {
                target_id: node,
                type_id: CMD_BUMP_PARAM,
                arg0: p.id as i64,
                arg1: clamped,
            })
        }
        ClientMsg::NodeCmd { node, cmd, a0, a1 } => {
            if cmd < 16 {
                return FrameAction::Drop(
                    "node_cmd: universal range (<16) must use typed messages",
                );
            }
            if !session.nodes.iter().any(|n| n.id == node) {
                return FrameAction::Drop("node_cmd: unknown node");
            }
            FrameAction::Command(NodeCommand {
                target_id: node,
                type_id: cmd,
                arg0: a0,
                arg1: a1,
            })
        }
        _ => unreachable!("resolve_semantic called with a non-semantic-plane message"),
    }
}

/// Route one decoded client message. Pure; runs on WS read threads.
pub fn route_frame(msg: ClientMsg, session: &SessionInfo) -> FrameAction {
    match msg {
        ClientMsg::Ping { ts } => FrameAction::Pong(ts),
        ClientMsg::Hello { .. } => FrameAction::Drop("duplicate hello"),
        ClientMsg::PadPres { .. } => FrameAction::Drop("pad_pres is W1+"),
        ClientMsg::GetViewMeta { track_id, nonce } => {
            match session.view_registry.assemble(track_id, nonce, &session.nodes) {
                Some(msg) => FrameAction::ViewMeta(msg),
                None => FrameAction::Drop("get_view_meta: unknown track"),
            }
        }
        ref m @ (ClientMsg::SetParam { .. }
        | ClientMsg::BumpParam { .. }
        | ClientMsg::NodeCmd { .. }) => resolve_semantic(m.clone(), session),
        ref m @ (ClientMsg::PadDown { .. }
        | ClientMsg::PadUp { .. }
        | ClientMsg::Enc { .. }
        | ClientMsg::EncPush { .. }) => match client_msg_to_surface_event(m) {
            Some(ev) => FrameAction::Emit(ev),
            None => FrameAction::Drop("control id out of range"),
        },
    }
}

// ── Listener ──────────────────────────────────────────────────────────────────

/// Spawn the WS accept thread. Each accepted connection gets its own I/O
/// thread; slot allocation and the token gate happen there.
pub fn spawn_listener(
    ws_port: u16,
    session: Arc<SessionInfo>,
    clients: Arc<Mutex<ClientTable>>,
    pool: ProducerPool,
    cmd_tx: mpsc::Sender<NodeCommand>,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", ws_port))?;
    std::thread::Builder::new()
        .name("antiphon-accept".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let session = Arc::clone(&session);
                let clients = Arc::clone(&clients);
                let pool = Arc::clone(&pool);
                let cmd_tx = cmd_tx.clone();
                let _ = std::thread::Builder::new()
                    .name("antiphon-client".into())
                    .spawn(move || client_session(stream, session, clients, pool, cmd_tx));
            }
        })?;
    Ok(())
}

fn send_msg(ws: &mut WebSocket<TcpStream>, msg: &ServerMsg) -> bool {
    match serde_json::to_string(msg) {
        Ok(frame) => ws.send(Message::Text(frame)).is_ok(),
        Err(_) => false,
    }
}

fn send_bye(ws: &mut WebSocket<TcpStream>, reason: &str) {
    send_msg(
        ws,
        &ServerMsg::Bye {
            reason: reason.to_owned(),
        },
    );
    let _ = ws.close(None);
    // Give the close frame a chance to flush before the socket drops.
    let _ = ws.flush();
}

/// RAII cleanup for an allocated client slot: returns the ring producer to
/// the pool *before* freeing the table slot (a reconnecting client can grab
/// the slot the instant it frees), and runs on unwind too, so a panicking
/// client thread cannot leak its producer and poison the slot forever.
struct SlotGuard {
    slot: usize,
    producer: Option<rtrb::Producer<SurfaceEvent>>,
    clients: Arc<Mutex<ClientTable>>,
    pool: ProducerPool,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if let (Some(producer), Ok(mut p)) = (self.producer.take(), self.pool.lock()) {
            p[self.slot] = Some(producer);
        }
        if let Ok(mut t) = self.clients.lock() {
            t.free(self.slot);
        }
        eprintln!("[antiphon] client disconnected (slot {})", self.slot);
    }
}

/// Full lifecycle of one client connection: WS handshake → token gate → slot
/// allocation → welcome → steady-state I/O loop → cleanup (via `SlotGuard`).
fn client_session(
    stream: TcpStream,
    session: Arc<SessionInfo>,
    clients: Arc<Mutex<ClientTable>>,
    pool: ProducerPool,
    cmd_tx: mpsc::Sender<NodeCommand>,
) {
    // Bound the WS upgrade + hello so a stalled connection can't pin the
    // thread forever.
    let _ = stream.set_read_timeout(Some(HELLO_TIMEOUT));
    let Ok(mut ws) = tungstenite::accept(stream) else {
        return;
    };

    // First frame: hello with the right token.
    let first = match ws.read() {
        Ok(Message::Text(t)) => t,
        Ok(_) => {
            send_bye(&mut ws, "expected hello");
            return;
        }
        Err(_) => {
            send_bye(&mut ws, "hello timeout");
            return;
        }
    };
    let client_name = match evaluate_hello(&first, &session.token) {
        HandshakeDecision::Accept { client } => client,
        HandshakeDecision::Reject { reason } => {
            eprintln!("[antiphon] rejected connection: {reason}");
            send_bye(&mut ws, reason);
            return;
        }
    };

    // Slot allocation (also flags the slot for kerygma's full-surface replay).
    let (outbound_tx, outbound_rx) = mpsc::channel::<String>();
    let slot = match clients
        .lock()
        .ok()
        .and_then(|mut t| t.allocate(outbound_tx))
    {
        Some(idx) => idx,
        None => {
            eprintln!("[antiphon] refused client (all {MAX_CLIENTS} slots busy)");
            send_bye(&mut ws, "full");
            return;
        }
    };
    let mut guard = SlotGuard {
        slot,
        producer: pool.lock().ok().and_then(|mut p| p[slot].take()),
        clients: Arc::clone(&clients),
        pool: Arc::clone(&pool),
    };
    if guard.producer.is_none() {
        // Producer missing for an allocated slot — should be impossible;
        // the guard releases the slot on return.
        send_bye(&mut ws, "internal error");
        return;
    }

    eprintln!("[antiphon] client \"{client_name}\" connected (slot {slot})");
    send_msg(
        &mut ws,
        &ServerMsg::Welcome {
            protocol: PROTOCOL_VERSION,
            device_id: session.device_id,
            nodes: session.nodes.clone(),
            transport: session.transport.clone(),
        },
    );

    // Steady state: alternate short-timeout reads with outbound drains.
    // Any `return` (or panic) from here on runs SlotGuard's cleanup.
    let _ = ws.get_ref().set_read_timeout(Some(IO_POLL));
    loop {
        // Outbound first so LED batches queued while we slept go out now.
        loop {
            match outbound_rx.try_recv() {
                Ok(frame) => {
                    if ws.send(Message::Text(frame)).is_err() {
                        return;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        match ws.read() {
            Ok(Message::Text(frame)) => match serde_json::from_str::<ClientMsg>(&frame) {
                Ok(msg) => match route_frame(msg, &session) {
                    FrameAction::Emit(ev) => {
                        let full = guard
                            .producer
                            .as_mut()
                            .map(|p| p.push(ev).is_err())
                            .unwrap_or(true);
                        if full {
                            eprintln!("[antiphon] slot {slot} ring full; event dropped");
                        }
                    }
                    FrameAction::Pong(ts) => {
                        if !send_msg(&mut ws, &ServerMsg::Pong { ts }) {
                            return;
                        }
                    }
                    FrameAction::Command(cmd) => {
                        let _ = cmd_tx.send(cmd);
                    }
                    FrameAction::ViewMeta(msg) => {
                        if !send_msg(&mut ws, &msg) {
                            return;
                        }
                    }
                    FrameAction::Drop(why) => {
                        eprintln!("[antiphon] dropped frame from slot {slot}: {why}");
                    }
                },
                Err(_) => eprintln!("[antiphon] malformed frame from slot {slot}"),
            },
            // Binary/ping/pong frames: ignore (tungstenite auto-answers pings).
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {}
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session with no registered nodes — enough for the tests below that
    /// only exercise the surface-plane / reserved-message branches, where
    /// `resolve_semantic` is expected to drop for lack of a resolvable node.
    fn empty_session() -> SessionInfo {
        SessionInfo {
            token: String::new(),
            device_id: 0,
            nodes: vec![],
            transport: TransportSummary {
                playing: false,
                bpm: 120.0,
            },
            view_registry: ViewRegistry {
                rules: HashMap::new(),
                chains: Vec::new(),
            },
        }
    }

    #[test]
    fn hello_bad_token_gets_bye() {
        let d = evaluate_hello(
            r#"{"t":"hello","token":"wrong","client":"theoria-web/0.1"}"#,
            "right",
        );
        assert_eq!(
            d,
            HandshakeDecision::Reject {
                reason: "bad token"
            }
        );

        let d = evaluate_hello(
            r#"{"t":"hello","token":"right","client":"theoria-web/0.1"}"#,
            "right",
        );
        assert_eq!(
            d,
            HandshakeDecision::Accept {
                client: "theoria-web/0.1".into()
            }
        );
    }

    #[test]
    fn open_mode_accepts_any_client_token() {
        // Empty expected token = open mode: a stale code remembered from a
        // previous --token run must not bounce the client (2026-07-10).
        for raw in [
            r#"{"t":"hello","token":"","client":"c"}"#,
            r#"{"t":"hello","token":"972461","client":"c"}"#,
            r#"{"t":"hello","token":"deadbeefdeadbeefdeadbeefdeadbeef","client":"c"}"#,
        ] {
            let d = evaluate_hello(raw, "");
            assert_eq!(
                d,
                HandshakeDecision::Accept { client: "c".into() },
                "must accept: {raw}"
            );
        }
    }

    #[test]
    fn hello_must_be_first_frame() {
        // A valid protocol message that is not hello is rejected.
        let d = evaluate_hello(r#"{"t":"pad_down","id":13,"vel":65535}"#, "tok");
        assert_eq!(
            d,
            HandshakeDecision::Reject {
                reason: "hello must be first frame"
            }
        );
        // Garbage is rejected without panicking.
        let d = evaluate_hello("garbage", "tok");
        assert_eq!(
            d,
            HandshakeDecision::Reject {
                reason: "malformed hello"
            }
        );
    }

    #[test]
    fn unresolvable_semantic_and_reserved_variants_parse_but_are_dropped() {
        // The semantic-plane variants parse (protocol reserves the shape) and
        // drop against a session with no matching node/param rather than
        // erroring or emitting. The reserved PadPres variant drops
        // unconditionally.
        let session = empty_session();
        let msg: ClientMsg =
            serde_json::from_str(r#"{"t":"set_param","node":20,"param":"cutoff","v":0.5}"#)
                .expect("set_param must parse");
        assert!(matches!(route_frame(msg, &session), FrameAction::Drop(_)));

        for raw in [
            r#"{"t":"bump_param","node":20,"param":"cutoff","delta":0.01}"#,
            r#"{"t":"node_cmd","node":10,"cmd":16,"a0":3,"a1":0}"#,
            r#"{"t":"pad_pres","id":13,"v":41000}"#,
        ] {
            let msg: ClientMsg = serde_json::from_str(raw).expect("variant must parse");
            assert!(
                matches!(route_frame(msg, &session), FrameAction::Drop(_)),
                "must drop: {raw}"
            );
        }
    }

    #[test]
    fn route_frame_emits_encoder_events() {
        // The W0 "encoders are W1" gate is open: enc/enc_push route to the
        // surface plane like pads (W1 C3 gap, found 2026-07-10).
        let session = empty_session();
        assert!(matches!(
            route_frame(ClientMsg::Enc { id: 90, delta: -3 }, &session),
            FrameAction::Emit(SurfaceEvent::EncoderChanged {
                id: 90,
                delta: -3,
                ..
            })
        ));
        assert!(matches!(
            route_frame(
                ClientMsg::EncPush {
                    id: 97,
                    pressed: true
                },
                &session
            ),
            FrameAction::Emit(SurfaceEvent::EncoderPush {
                id: 97,
                pressed: true
            })
        ));
    }

    #[test]
    fn bump_param_delta_cap_is_range_relative() {
        // A fixed absolute cap froze wide-range params (200-8000 Hz cutoff);
        // the cap is a fraction of the declared range (found 2026-07-10).
        let session = SessionInfo {
            token: String::new(),
            device_id: 106,
            nodes: vec![crate::protocol::NodeSummary {
                id: 40,
                type_tag: "filter".into(),
                name: "Filter".into(),
                has_view: false,
                params: vec![crate::protocol::ParamSummary {
                    id: 7,
                    name: "cutoff".into(),
                    min: 200.0,
                    max: 8000.0,
                    default: 4000.0,
                }],
            }],
            transport: TransportSummary {
                playing: false,
                bpm: 120.0,
            },
            view_registry: ViewRegistry { rules: HashMap::new(), chains: Vec::new() },
        };
        // A sane per-detent delta passes through unclamped…
        let FrameAction::Command(cmd) = route_frame(
            ClientMsg::BumpParam {
                node: 40,
                param: "cutoff".into(),
                delta: 30.5,
            },
            &session,
        ) else {
            panic!("expected command")
        };
        assert_eq!(cmd.arg1, 30.5, "in-range delta must not be clamped");
        // …and an absurd one is bounded to half the range, not to 0.5.
        let FrameAction::Command(cmd) = route_frame(
            ClientMsg::BumpParam {
                node: 40,
                param: "cutoff".into(),
                delta: -99999.0,
            },
            &session,
        ) else {
            panic!("expected command")
        };
        assert_eq!(cmd.arg1, -3900.0, "cap is (max-min) * 0.5");
    }

    #[test]
    fn route_frame_emits_pad_events_and_pongs() {
        let session = empty_session();
        assert!(matches!(
            route_frame(ClientMsg::PadDown { id: 13, vel: 65535 }, &session),
            FrameAction::Emit(SurfaceEvent::PadPressed { id: 13, .. })
        ));
        assert!(matches!(
            route_frame(ClientMsg::PadUp { id: 70 }, &session),
            FrameAction::Emit(SurfaceEvent::ButtonReleased { id: 70 })
        ));
        assert!(matches!(
            route_frame(ClientMsg::Ping { ts: 42.5 }, &session),
            FrameAction::Pong(ts) if ts == 42.5
        ));
        assert!(matches!(
            route_frame(ClientMsg::PadDown { id: 99, vel: 1 }, &session),
            FrameAction::Drop("control id out of range")
        ));
    }
}

#[cfg(test)]
mod semantic_tests {
    use super::*;
    use crate::protocol::ParamSummary;

    fn session_with_cutoff() -> SessionInfo {
        SessionInfo {
            token: String::new(),
            device_id: 0,
            nodes: vec![NodeSummary {
                id: 20,
                type_tag: "test".into(),
                name: "test".into(),
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
                playing: false,
                bpm: 120.0,
            },
            view_registry: ViewRegistry {
                rules: HashMap::new(),
                chains: Vec::new(),
            },
        }
    }

    #[test]
    fn set_param_resolves_name_to_id() {
        let session = session_with_cutoff();
        let msg = ClientMsg::SetParam {
            node: 20,
            param: "cutoff".into(),
            v: 0.4,
        };
        let FrameAction::Command(cmd) = route_frame(msg, &session) else {
            panic!("expected Command");
        };
        assert_eq!(cmd.target_id, 20);
        assert_eq!(cmd.type_id, CMD_SET_PARAM);
        assert_eq!(cmd.arg0, 123);
        assert_eq!(cmd.arg1, 0.4);
    }

    #[test]
    fn set_param_clamps_value() {
        let session = session_with_cutoff();
        let msg = ClientMsg::SetParam {
            node: 20,
            param: "cutoff".into(),
            v: 5.0,
        };
        let FrameAction::Command(cmd) = route_frame(msg, &session) else {
            panic!("expected Command");
        };
        assert_eq!(cmd.arg1, 1.0, "clamped to declared max");

        let msg = ClientMsg::SetParam {
            node: 20,
            param: "cutoff".into(),
            v: -1.0,
        };
        let FrameAction::Command(cmd) = route_frame(msg, &session) else {
            panic!("expected Command");
        };
        assert_eq!(cmd.arg1, 0.0, "clamped to declared min");
    }

    #[test]
    fn bump_param_clamps_delta() {
        let session = session_with_cutoff();
        let msg = ClientMsg::BumpParam {
            node: 20,
            param: "cutoff".into(),
            delta: 2.0,
        };
        let FrameAction::Command(cmd) = route_frame(msg, &session) else {
            panic!("expected Command");
        };
        assert_eq!(cmd.type_id, CMD_BUMP_PARAM);
        assert_eq!(cmd.arg0, 123);
        assert_eq!(
            cmd.arg1, 0.5,
            "clamped to the per-message rate cap, not the param range"
        );

        let msg = ClientMsg::BumpParam {
            node: 20,
            param: "cutoff".into(),
            delta: -2.0,
        };
        let FrameAction::Command(cmd) = route_frame(msg, &session) else {
            panic!("expected Command");
        };
        assert_eq!(cmd.arg1, -0.5);
    }

    #[test]
    fn unknown_node_or_param_is_dropped() {
        let session = session_with_cutoff();
        let msg = ClientMsg::SetParam {
            node: 99,
            param: "cutoff".into(),
            v: 0.5,
        };
        assert!(matches!(route_frame(msg, &session), FrameAction::Drop(_)));

        let msg = ClientMsg::SetParam {
            node: 20,
            param: "nope".into(),
            v: 0.5,
        };
        assert!(matches!(route_frame(msg, &session), FrameAction::Drop(_)));
    }

    #[test]
    fn node_cmd_rejects_universal_range() {
        let session = session_with_cutoff();
        let msg = ClientMsg::NodeCmd {
            node: 20,
            cmd: 0,
            a0: 3,
            a1: 0.0,
        };
        assert!(matches!(route_frame(msg, &session), FrameAction::Drop(_)));

        let msg = ClientMsg::NodeCmd {
            node: 20,
            cmd: 15,
            a0: 3,
            a1: 0.0,
        };
        assert!(matches!(route_frame(msg, &session), FrameAction::Drop(_)));

        let msg = ClientMsg::NodeCmd {
            node: 20,
            cmd: 16,
            a0: 3,
            a1: 7.0,
        };
        let FrameAction::Command(cmd) = route_frame(msg, &session) else {
            panic!("expected Command pass-through");
        };
        assert_eq!(cmd.target_id, 20);
        assert_eq!(cmd.type_id, 16);
        assert_eq!(cmd.arg0, 3);
        assert_eq!(cmd.arg1, 7.0);
    }

    #[test]
    fn node_cmd_unknown_node_dropped() {
        let session = session_with_cutoff();
        let msg = ClientMsg::NodeCmd {
            node: 99,
            cmd: 16,
            a0: 0,
            a1: 0.0,
        };
        assert!(matches!(route_frame(msg, &session), FrameAction::Drop(_)));
    }
}
