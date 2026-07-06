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

use paraclete_node_api::SurfaceEvent;
use tungstenite::{Message, WebSocket};

use crate::kerygma::{ClientTable, MAX_CLIENTS};
use crate::protocol::{ClientMsg, NodeSummary, ServerMsg, TransportSummary, PROTOCOL_VERSION};
use crate::surface::client_msg_to_surface_event;

/// The first frame must be a valid `hello` within this window.
const HELLO_TIMEOUT: Duration = Duration::from_secs(3);
/// Read timeout for the steady-state I/O loop; bounds outbound-frame latency.
const IO_POLL: Duration = Duration::from_millis(15);

/// Immutable per-session data shared with every client thread.
pub struct SessionInfo {
    pub token: String,
    pub device_id: u32,
    pub nodes: Vec<NodeSummary>,
    pub transport: TransportSummary,
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
pub fn evaluate_hello(frame: &str, expected_token: &str) -> HandshakeDecision {
    match serde_json::from_str::<ClientMsg>(frame) {
        Ok(ClientMsg::Hello { token, client }) => {
            if token == expected_token {
                HandshakeDecision::Accept { client }
            } else {
                HandshakeDecision::Reject { reason: "bad token" }
            }
        }
        Ok(_) => HandshakeDecision::Reject { reason: "hello must be first frame" },
        Err(_) => HandshakeDecision::Reject { reason: "malformed hello" },
    }
}

/// What the I/O loop should do with a post-handshake frame.
#[derive(Debug)]
pub enum FrameAction {
    /// Push this event into the client's slot ring.
    Emit(SurfaceEvent),
    /// Reply with `pong` echoing the timestamp.
    Pong(f64),
    /// Log and drop (W1 variants, reserved messages, out-of-range ids).
    Drop(&'static str),
}

/// Route one decoded client message. Pure; runs on WS read threads.
pub fn route_frame(msg: ClientMsg) -> FrameAction {
    match msg {
        ClientMsg::Ping { ts } => FrameAction::Pong(ts),
        ClientMsg::Hello { .. } => FrameAction::Drop("duplicate hello"),
        // Reserved [W1+]: poly-aftertouch pressure. Parse-and-drop at W0.
        ClientMsg::PadPres { .. } => FrameAction::Drop("pad_pres is W1+"),
        // Surface plane, gated to W1 (descriptor declares the encoders now;
        // delivery waits so W0 builds nothing early).
        ClientMsg::Enc { .. } | ClientMsg::EncPush { .. } => FrameAction::Drop("encoders are W1"),
        // Semantic plane is W1.
        ClientMsg::SetParam { .. } | ClientMsg::BumpParam { .. } | ClientMsg::NodeCmd { .. } => {
            FrameAction::Drop("semantic plane is W1")
        }
        ref m @ (ClientMsg::PadDown { .. } | ClientMsg::PadUp { .. }) => {
            match client_msg_to_surface_event(m) {
                Some(ev) => FrameAction::Emit(ev),
                None => FrameAction::Drop("pad id out of range"),
            }
        }
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
                let _ = std::thread::Builder::new()
                    .name("antiphon-client".into())
                    .spawn(move || client_session(stream, session, clients, pool));
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
    send_msg(ws, &ServerMsg::Bye { reason: reason.to_owned() });
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
    let slot = match clients.lock().ok().and_then(|mut t| t.allocate(outbound_tx)) {
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
                Ok(msg) => match route_frame(msg) {
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

    #[test]
    fn hello_bad_token_gets_bye() {
        let d = evaluate_hello(
            r#"{"t":"hello","token":"wrong","client":"theoria-web/0.1"}"#,
            "right",
        );
        assert_eq!(d, HandshakeDecision::Reject { reason: "bad token" });

        let d = evaluate_hello(
            r#"{"t":"hello","token":"right","client":"theoria-web/0.1"}"#,
            "right",
        );
        assert_eq!(d, HandshakeDecision::Accept { client: "theoria-web/0.1".into() });
    }

    #[test]
    fn hello_must_be_first_frame() {
        // A valid protocol message that is not hello is rejected.
        let d = evaluate_hello(r#"{"t":"pad_down","id":13,"vel":65535}"#, "tok");
        assert_eq!(d, HandshakeDecision::Reject { reason: "hello must be first frame" });
        // Garbage is rejected without panicking.
        let d = evaluate_hello("garbage", "tok");
        assert_eq!(d, HandshakeDecision::Reject { reason: "malformed hello" });
    }

    #[test]
    fn w1_variants_parse_but_are_dropped() {
        // The [W1] semantic-plane variants parse (protocol reserves them) and
        // the router drops them at W0 rather than erroring or emitting.
        let msg: ClientMsg =
            serde_json::from_str(r#"{"t":"set_param","node":20,"param":"cutoff","v":0.5}"#)
                .expect("set_param must parse at W0");
        assert!(matches!(route_frame(msg), FrameAction::Drop(_)));

        for raw in [
            r#"{"t":"bump_param","node":20,"param":"cutoff","delta":0.01}"#,
            r#"{"t":"node_cmd","node":10,"cmd":16,"a0":3,"a1":0}"#,
            r#"{"t":"enc","id":90,"delta":-3}"#,
            r#"{"t":"enc_push","id":90,"pressed":true}"#,
            r#"{"t":"pad_pres","id":13,"v":41000}"#,
        ] {
            let msg: ClientMsg = serde_json::from_str(raw).expect("W1 variant must parse");
            assert!(matches!(route_frame(msg), FrameAction::Drop(_)), "must drop: {raw}");
        }
    }

    #[test]
    fn route_frame_emits_pad_events_and_pongs() {
        assert!(matches!(
            route_frame(ClientMsg::PadDown { id: 13, vel: 65535 }),
            FrameAction::Emit(SurfaceEvent::PadPressed { id: 13, .. })
        ));
        assert!(matches!(
            route_frame(ClientMsg::PadUp { id: 70 }),
            FrameAction::Emit(SurfaceEvent::ButtonReleased { id: 70 })
        ));
        assert!(matches!(route_frame(ClientMsg::Ping { ts: 42.5 }), FrameAction::Pong(ts) if ts == 42.5));
        assert!(matches!(
            route_frame(ClientMsg::PadDown { id: 99, vel: 1 }),
            FrameAction::Drop("pad id out of range")
        ));
    }
}
