// SPDX-License-Identifier: GPL-3.0-or-later
//! Antiphon — the Paraclete interface server (ADR-031).
//!
//! Presentation-agnostic: Theoria clients (web tablet, terminal) speak
//! protocol v0 over WebSocket and manifest in the graph as one
//! `TheoriaSurfaceNode`, a peer to the physical `LaunchpadNode`. LED
//! broadcast and client replay live in `kerygma`; wire names stay plain
//! (naming policy, `design/interface-plan.md`).
//!
//! No tokio: blocking `tungstenite` + `rtrb` rings + one thread per client,
//! the same pattern the MIDI stack uses (`midir` callbacks + SPSC).

pub mod http;
pub mod kerygma;
pub mod protocol;
pub mod server;
pub mod surface;
pub mod view;

use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use http::StaticSource;
use kerygma::{ClientTable, Kerygma, MAX_CLIENTS};
use paraclete_node_api::{NodeCommand, StateBusHandle, StateBusValue};
use protocol::{ContextSlot, NodeSummary, ServerMsg, StateUpdate, TransportSummary};
use server::SessionInfo;
use surface::TheoriaSurfaceNode;

/// Coalescing window for the state mirror (w1-interfaces.md §Commit 2).
const STATE_MIRROR_FLUSH_MS: u64 = 33;
/// Max `StateUpdate`s per `state` frame; overflow flushes immediately and
/// continues within the same `pump()` call (no drops).
const STATE_MIRROR_BATCH_CAP: usize = 256;

/// Default HTTP port (WebSocket listens on `port + 1`).
/// 7274 = "PARA" on a phone keypad.
pub const DEFAULT_PORT: u16 = 7274;

pub struct AntiphonConfig {
    /// HTTP port for the static client bundle; the WS server binds `port + 1`.
    pub port: u16,
    /// Session token; gates the WS handshake. The app generates and prints it.
    pub token: String,
    /// Source of the Theoria client bundle; `None` disables the HTTP side
    /// (WS-only — useful for tests and headless drivers). `Some(Disk(_))` for
    /// dev (`--theoria-dir`); `Some(Embedded(_))` for the `embed-ui` release
    /// build (app-assembled, see `paraclete-app`'s `embed-ui` feature).
    pub static_dir: Option<StaticSource>,
    /// Node id the app will register the surface under (`welcome.device_id`).
    pub device_id: u32,
}

/// Main-thread handle to the running server. W1 extends this with the state
/// mirror pump and topology publishing.
pub struct AntiphonHandle {
    /// Printable client URL, token included.
    pub url: String,
    clients: Arc<Mutex<ClientTable>>,
    /// Last value sent to clients per numeric path (post-coercion). Diffing
    /// this shadow against `bus.iter()` each `pump()` is what turns a
    /// full state-bus scan into a change-only wire batch.
    state_shadow: HashMap<String, f64>,
    /// Coalesced diffs awaiting the next flush. A `HashMap` rather than a
    /// `Vec` so repeated changes to the same path within one coalescing
    /// window collapse to the latest value instead of both going out.
    pending: HashMap<String, f64>,
    /// Last value seen per `/context/*` path, for change detection. Stored
    /// as the raw `StateBusValue` (Int for `.../node`, Text for `.../param`)
    /// so equality comparison needs no coercion.
    context_shadow: HashMap<String, StateBusValue>,
    /// `None` until the first `pump()` call — the first pump always flushes
    /// (nothing to coalesce against yet); subsequent flushes gate on
    /// `STATE_MIRROR_FLUSH_MS` elapsed since this timestamp.
    last_flush_ms: Option<u64>,
    /// Set when any `/context/*` path changes; cleared only when the context
    /// snapshot is actually flushed. Must persist across pumps — a context
    /// change during a non-due pump updates the shadow (so the next pump does
    /// not re-detect it), so the pending flush has to be remembered here or
    /// the change would be silently lost.
    context_dirty: bool,
    /// Receiving end of the semantic-plane command channel: every client I/O
    /// thread holds a clone of the paired `Sender` and feeds it resolved
    /// `NodeCommand`s (w1-interfaces.md §Commit 3). Many producers, one
    /// consumer (the main loop) — `std::sync::mpsc`, not `rtrb`; this is off
    /// the audio thread so its allocation is fine.
    cmd_rx: mpsc::Receiver<NodeCommand>,
}

impl AntiphonHandle {
    pub fn client_count(&self) -> usize {
        self.clients.lock().map(|t| t.active_count()).unwrap_or(0)
    }

    /// Drain semantic-plane `NodeCommand`s resolved by client threads since
    /// the last call. Main-thread only; call once per main-loop iteration
    /// and forward each to `NodeConfigurator::send_command()`.
    pub fn drain_commands(&self) -> impl Iterator<Item = NodeCommand> + '_ {
        self.cmd_rx.try_iter()
    }

    /// Diff the state bus against the shadow, coalesce changes, and flush a
    /// `state` batch (and, if any `/context/*` path changed, a full `context`
    /// snapshot) at the `STATE_MIRROR_FLUSH_MS` cadence. Main-thread only;
    /// call once per main-loop iteration after `conf.process_main_thread()`.
    /// `now_ms` is a monotonic caller-supplied clock — antiphon does no
    /// clock reads of its own (w1-interfaces.md §Commit 2).
    pub fn pump(&mut self, bus: &StateBusHandle, now_ms: u64) {
        self.service_state_replays(bus);
        for (path, value) in bus.iter() {
            if path.starts_with("/context/") {
                let changed = self.context_shadow.get(path) != Some(value);
                if changed {
                    self.context_shadow.insert(path.to_string(), value.clone());
                    self.context_dirty = true;
                }
                continue;
            }

            // `/script/*` joined the mirror for the Theoria legibility phase
            // (s1.md F2/F8): profiles publish UI-relevant state there
            // (`/script/lp/selected`, `/script/lp/mode_n`) and clients need it
            // to show selection/mode set from any surface. Numeric values
            // only — same coercion rule as every other mirrored path.
            let is_mirrored_state_path = (path.starts_with("/node/")
                && (path.contains("/param/") || path.contains("/state/")))
                || path.starts_with("/transport/")
                || path.starts_with("/script/")
                || path.starts_with("/engine/");
            if !is_mirrored_state_path {
                continue;
            }

            // Rule 2: Text values are skipped entirely (steps bitfield,
            // track_name, `/script/lp/mode`) — the grid renders from LED
            // messages and names come from NodeSummary, not the numeric
            // mirror. Profiles that want a mode visible on clients publish a
            // numeric twin (`/script/lp/mode_n`).
            let coerced = match value {
                StateBusValue::Float(f) => *f,
                StateBusValue::Int(i) => *i as f64,
                StateBusValue::Bool(b) => {
                    if *b {
                        1.0
                    } else {
                        0.0
                    }
                }
                StateBusValue::Text(_) => continue,
            };

            let changed = self.state_shadow.get(path) != Some(&coerced);
            if !changed {
                continue;
            }
            self.state_shadow.insert(path.to_string(), coerced);
            self.pending.insert(path.to_string(), coerced);

            // Overflow: flush this batch immediately and keep diffing —
            // no path is ever dropped.
            if self.pending.len() >= STATE_MIRROR_BATCH_CAP {
                self.flush_state_batch();
            }
        }

        // `match` rather than `map_or`/`is_none_or`: the former trips a clippy
        // style lint, the latter isn't stable at this crate's MSRV (1.75).
        let due = match self.last_flush_ms {
            None => true,
            Some(last) => now_ms.saturating_sub(last) >= STATE_MIRROR_FLUSH_MS,
        };
        if due {
            self.flush_state_batch();
            if self.context_dirty {
                self.flush_context_snapshot(bus);
                self.context_dirty = false;
            }
            self.last_flush_ms = Some(now_ms);
        }
    }

    fn flush_state_batch(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let updates: Vec<StateUpdate> = self
            .pending
            .drain()
            .map(|(path, v)| StateUpdate { path, v })
            .collect();
        let Ok(frame) = serde_json::to_string(&ServerMsg::State { updates }) else {
            return;
        };
        if let Ok(table) = self.clients.lock() {
            table.send_to_all(&frame);
        }
    }

    /// Full state + context replay to clients that connected since the last
    /// pump. The diff mirror only sends changes, so without this a fresh
    /// client would show nothing until each value next changed (the state
    /// twin of kerygma's LED shadow replay).
    fn service_state_replays(&mut self, bus: &StateBusHandle) {
        let Ok(mut table) = self.clients.lock() else {
            return;
        };
        if !table.any_state_replay_pending() {
            return;
        }
        let state_frame = if self.state_shadow.is_empty() {
            None
        } else {
            let updates: Vec<StateUpdate> = self
                .state_shadow
                .iter()
                .map(|(path, v)| StateUpdate {
                    path: path.clone(),
                    v: *v,
                })
                .collect();
            serde_json::to_string(&ServerMsg::State { updates }).ok()
        };
        let context_frame = build_context_frame(bus);
        table.for_each_state_replay_pending(|slot| {
            if let Some(f) = &state_frame {
                let _ = slot.sender.send(f.clone());
            }
            if let Some(f) = &context_frame {
                let _ = slot.sender.send(f.clone());
            }
        });
    }

    /// Full 8-slot `/context/*` snapshot: emitted whenever any context path
    /// changed since the last pump. Reads directly from the bus rather than
    /// the shadow so the snapshot is always complete, not just the diff.
    fn flush_context_snapshot(&self, bus: &StateBusHandle) {
        let Some(frame) = build_context_frame(bus) else {
            return;
        };
        if let Ok(table) = self.clients.lock() {
            table.send_to_all(&frame);
        }
    }

    /// Broadcast a refreshed topology snapshot (same shape as `welcome.nodes`)
    /// after `apply_patch()` changes the graph. No live runtime call site
    /// exists yet at W1 (dynamic topology is exercised in tests only); this
    /// is the hook for when `apply_patch` is wired to a trigger.
    pub fn publish_topology(&self, nodes: Vec<NodeSummary>) {
        let Ok(frame) = serde_json::to_string(&ServerMsg::Topology { nodes }) else {
            return;
        };
        if let Ok(table) = self.clients.lock() {
            table.send_to_all(&frame);
        }
    }
}

/// Serialize the full `/context/*` snapshot from the bus into a `context`
/// frame; `None` when serialization fails. Shared by the periodic flush and
/// the new-client replay.
fn build_context_frame(bus: &StateBusHandle) -> Option<String> {
    let mut halves: HashMap<&str, (Option<i64>, Option<&str>)> = HashMap::new();
    for (path, value) in bus.iter() {
        let Some(rest) = path.strip_prefix("/context/") else {
            continue;
        };
        if let Some(key) = rest.strip_suffix("/node") {
            if let StateBusValue::Int(i) = value {
                halves.entry(key).or_default().0 = Some(*i);
            }
        } else if let Some(key) = rest.strip_suffix("/param") {
            if let StateBusValue::Text(s) = value {
                halves.entry(key).or_default().1 = Some(s.as_str());
            }
        }
    }

    let mut slots: Vec<ContextSlot> = halves
        .into_iter()
        .filter_map(|(key, (node, param))| {
            let node = node?;
            let param = param?;
            // Boring documented choice (flagged for C4 validation): the
            // enc id is the trailing integer in the encoder_key used by
            // publish_context() (e.g. "encoder_3" -> 3). This assumes a
            // one profile-encoder-key-per-slot convention; it may need a
            // defined map once the web encoder row binds ids 90-97.
            let enc = trailing_int(key)?;
            Some(ContextSlot {
                enc,
                node: node as u32,
                param: param.to_string(),
            })
        })
        .collect();
    slots.sort_by_key(|s| s.enc);

    serde_json::to_string(&ServerMsg::Context { slots }).ok()
}

/// Parse the trailing base-10 integer off a context key, e.g. `"encoder_3"`
/// -> `Some(3)`. `None` if the key has no trailing digits.
fn trailing_int(key: &str) -> Option<u32> {
    let digit_count = key.chars().rev().take_while(|c| c.is_ascii_digit()).count();
    if digit_count == 0 {
        return None;
    }
    key[key.len() - digit_count..].parse().ok()
}

pub struct AntiphonServer;

impl AntiphonServer {
    /// Bind the listeners and spawn the server threads.
    ///
    /// Returns the surface node (for `add_surface`) and the main-thread
    /// handle. `nodes`/`transport` are snapshotted into every `welcome` —
    /// the app assembles them from its cap-doc cache; antiphon never talks
    /// to the configurator.
    pub fn spawn(
        config: AntiphonConfig,
        nodes: Vec<NodeSummary>,
        transport: TransportSummary,
        view_registry: crate::view::ViewRegistry,
    ) -> std::io::Result<(TheoriaSurfaceNode, AntiphonHandle)> {
        let clients = Arc::new(Mutex::new(ClientTable::new()));
        let (node, producers) = TheoriaSurfaceNode::new(Kerygma::new(Arc::clone(&clients)));
        debug_assert_eq!(producers.len(), MAX_CLIENTS);
        let pool: server::ProducerPool =
            Arc::new(Mutex::new(producers.into_iter().map(Some).collect()));

        let session = Arc::new(SessionInfo {
            token: config.token.clone(),
            device_id: config.device_id,
            nodes,
            transport,
            view_registry,
        });
        let (cmd_tx, cmd_rx) = mpsc::channel::<NodeCommand>();
        server::spawn_listener(config.port + 1, session, Arc::clone(&clients), pool, cmd_tx)?;
        if let Some(dir) = config.static_dir {
            http::spawn_http(dir, config.port)?;
        }

        // Empty token = trusted-open mode: emit a bare URL (no `?t=`). The
        // client sends `token: ""` when the query param is absent, which the
        // handshake accepts against an empty expected token.
        let url = if config.token.is_empty() {
            format!("http://{}:{}/", lan_ip(), config.port)
        } else {
            format!("http://{}:{}/?t={}", lan_ip(), config.port, config.token)
        };
        Ok((
            node,
            AntiphonHandle {
                url,
                clients,
                state_shadow: HashMap::new(),
                pending: HashMap::new(),
                context_shadow: HashMap::new(),
                last_flush_ms: None,
                context_dirty: false,
                cmd_rx,
            },
        ))
    }
}

/// Best-effort LAN address for the printed URL. The UDP connect sends no
/// packets — it only asks the OS which interface would route out.
fn lan_ip() -> String {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("192.168.255.255:80")?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ClientMsg, ServerMsg};
    use std::net::TcpStream;
    use std::time::Duration;
    use tungstenite::Message;

    /// End-to-end smoke over real localhost sockets. `#[ignore]`-tagged like
    /// the clap-validator test; run with `cargo test -p paraclete-antiphon -- --ignored`.
    #[test]
    #[ignore]
    fn smoke_ws_session_over_localhost() {
        let port = 47274; // fixed test port; WS at 47275
        let config = AntiphonConfig {
            port,
            token: "cafebabe".into(),
            static_dir: None,
            device_id: 106,
        };
        let (mut node, handle) = AntiphonServer::spawn(
            config,
            vec![],
            TransportSummary {
                playing: false,
                bpm: 120.0,
            },
            crate::view::ViewRegistry {
                rules: HashMap::new(),
                chains: Vec::new(),
                node_infos: HashMap::new(),
            },
        )
        .expect("spawn");
        let mut out_handle =
            paraclete_node_api::Surface::take_output_handle(&mut node).expect("handle");

        let stream = TcpStream::connect(("127.0.0.1", port + 1)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (mut ws, _) =
            tungstenite::client("ws://127.0.0.1:47275/", stream).expect("ws handshake");

        // hello → welcome
        let hello = serde_json::to_string(&ClientMsg::Hello {
            token: "cafebabe".into(),
            client: "smoke/0".into(),
        })
        .unwrap();
        ws.send(Message::Text(hello)).unwrap();
        let Message::Text(frame) = ws.read().unwrap() else {
            panic!("expected text")
        };
        let ServerMsg::Welcome {
            protocol,
            device_id,
            ..
        } = serde_json::from_str(&frame).unwrap()
        else {
            panic!("expected welcome, got {frame}")
        };
        assert_eq!(protocol, 0);
        assert_eq!(device_id, 106);
        assert_eq!(handle.client_count(), 1);

        // Full-surface replay arrives once the main-thread handle ticks.
        out_handle.tick();
        let Message::Text(frame) = ws.read().unwrap() else {
            panic!("expected text")
        };
        assert!(matches!(
            serde_json::from_str::<ServerMsg>(&frame).unwrap(),
            ServerMsg::Led { .. }
        ));

        // ping → pong
        ws.send(Message::Text(r#"{"t":"ping","ts":7.5}"#.into()))
            .unwrap();
        let Message::Text(frame) = ws.read().unwrap() else {
            panic!("expected text")
        };
        assert_eq!(
            serde_json::from_str::<ServerMsg>(&frame).unwrap(),
            ServerMsg::Pong { ts: 7.5 }
        );

        ws.close(None).ok();
    }
}

#[cfg(test)]
mod pump_tests {
    use super::*;
    use protocol::ServerMsg;
    use std::sync::mpsc;

    /// Build a handle wired to one test client; returns the frame receiver.
    fn test_handle() -> (AntiphonHandle, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel();
        let mut table = ClientTable::new();
        table.allocate(tx).expect("one slot");
        let (_cmd_tx, cmd_rx) = mpsc::channel();
        let handle = AntiphonHandle {
            url: String::new(),
            clients: Arc::new(Mutex::new(table)),
            state_shadow: HashMap::new(),
            pending: HashMap::new(),
            context_shadow: HashMap::new(),
            last_flush_ms: None,
            context_dirty: false,
            cmd_rx,
        };
        (handle, rx)
    }

    fn drain(rx: &mpsc::Receiver<String>) -> Vec<ServerMsg> {
        let mut msgs = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            msgs.push(serde_json::from_str(&frame).expect("valid ServerMsg frame"));
        }
        msgs
    }

    fn state_updates(msgs: &[ServerMsg]) -> Vec<protocol::StateUpdate> {
        msgs.iter()
            .filter_map(|m| match m {
                ServerMsg::State { updates } => Some(updates.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn state_mirror_diffs_only_changed_paths() {
        let (mut h, rx) = test_handle();
        let mut bus = StateBusHandle::new();
        for i in 0..5 {
            bus.write(
                &format!("/node/{i}/param/cutoff"),
                StateBusValue::Float(i as f64),
            );
        }
        // First pump flushes all 5 (shadow empty → everything changed).
        h.pump(&bus, 0);
        assert_eq!(
            state_updates(&drain(&rx)).len(),
            5,
            "first pump mirrors all paths"
        );

        // Change exactly 2 of them; past the flush window → one batch of 2.
        bus.write("/node/1/param/cutoff", StateBusValue::Float(99.0));
        bus.write("/node/3/param/cutoff", StateBusValue::Float(88.0));
        h.pump(&bus, 40);
        let ups = state_updates(&drain(&rx));
        assert_eq!(ups.len(), 2, "only the 2 changed paths, got {ups:?}");
    }

    #[test]
    fn state_mirror_coalesces_within_window() {
        let (mut h, rx) = test_handle();
        let mut bus = StateBusHandle::new();
        bus.write("/transport/bpm", StateBusValue::Float(120.0));
        h.pump(&bus, 0); // initial flush
        let _ = drain(&rx);

        // Two changes inside the 33 ms window: no flush, coalesce to latest.
        bus.write("/transport/bpm", StateBusValue::Float(121.0));
        h.pump(&bus, 10);
        bus.write("/transport/bpm", StateBusValue::Float(122.0));
        h.pump(&bus, 20);
        assert!(
            drain(&rx).is_empty(),
            "no flush within the coalescing window"
        );

        // Past the window → exactly one frame carrying only the latest value.
        h.pump(&bus, 40);
        let ups = state_updates(&drain(&rx));
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0].v, 122.0, "coalesced to the latest value");
    }

    #[test]
    fn state_mirror_overflow_flushes_without_dropping() {
        let (mut h, rx) = test_handle();
        let mut bus = StateBusHandle::new();
        let n = STATE_MIRROR_BATCH_CAP * 2 + 5; // forces >= 3 batches
        for i in 0..n {
            bus.write(
                &format!("/node/{i}/param/x"),
                StateBusValue::Float(i as f64),
            );
        }
        h.pump(&bus, 0);
        let ups = state_updates(&drain(&rx));
        assert_eq!(
            ups.len(),
            n,
            "every path sent across multiple batches, none dropped"
        );
    }

    #[test]
    fn state_mirror_skips_text_values() {
        let (mut h, rx) = test_handle();
        let mut bus = StateBusHandle::new();
        bus.write("/node/1/state/steps", StateBusValue::Text("1010".into()));
        bus.write("/node/1/state/current_step", StateBusValue::Int(3));
        h.pump(&bus, 0);
        let ups = state_updates(&drain(&rx));
        assert!(
            ups.iter().all(|u| u.path != "/node/1/state/steps"),
            "Text path must never appear in the mirror: {ups:?}"
        );
        assert!(
            ups.iter()
                .any(|u| u.path == "/node/1/state/current_step" && u.v == 3.0),
            "numeric state path is mirrored"
        );
    }

    #[test]
    fn late_client_receives_full_state_and_context_replay() {
        // The mirror is diff-only; a client connecting after startup must be
        // sent the accumulated shadow or it shows nothing until each value
        // next changes (found 2026-07-10: fresh clients had no BPM/mode).
        let (mut h, rx) = test_handle();
        let mut bus = StateBusHandle::new();
        bus.write("/transport/bpm", StateBusValue::Float(140.0));
        bus.write("/script/lp/mode_n", StateBusValue::Int(0));
        bus.write("/context/encoder_0/node", StateBusValue::Int(20));
        bus.write(
            "/context/encoder_0/param",
            StateBusValue::Text("cutoff".into()),
        );
        h.pump(&bus, 0); // first client sees the initial flush
        let _ = drain(&rx);
        h.pump(&bus, 40); // steady state: nothing changed, nothing sent
        assert!(drain(&rx).is_empty());

        // Second client connects; values are all unchanged since its join.
        let (tx2, rx2) = mpsc::channel();
        h.clients
            .lock()
            .unwrap()
            .allocate(tx2)
            .expect("second slot");
        h.pump(&bus, 80);
        let msgs = drain(&rx2);
        let ups = state_updates(&msgs);
        assert!(
            ups.iter()
                .any(|u| u.path == "/transport/bpm" && u.v == 140.0),
            "replay carries transport state: {ups:?}"
        );
        assert!(
            ups.iter()
                .any(|u| u.path == "/script/lp/mode_n" && u.v == 0.0),
            "replay carries profile mode: {ups:?}"
        );
        assert!(
            msgs.iter()
                .any(|m| matches!(m, ServerMsg::Context { slots } if slots.len() == 1)),
            "replay carries the context snapshot: {msgs:?}"
        );

        // Replay happens exactly once; the first client got nothing extra.
        h.pump(&bus, 120);
        assert!(drain(&rx2).is_empty(), "no second replay");
        assert!(
            drain(&rx).is_empty(),
            "existing client unaffected by replay"
        );
    }

    #[test]
    fn state_mirror_includes_script_numeric_paths() {
        // Theoria legibility (s1.md F2/F8): numeric `/script/*` values reach
        // clients (selection, mode_n); Text `/script/*` values stay skipped.
        let (mut h, rx) = test_handle();
        let mut bus = StateBusHandle::new();
        bus.write("/script/lp/selected", StateBusValue::Int(2));
        bus.write("/script/lp/mode_n", StateBusValue::Int(1));
        bus.write("/script/lp/shift", StateBusValue::Bool(false));
        bus.write("/script/lp/mode", StateBusValue::Text("sequence".into()));
        h.pump(&bus, 0);
        let ups = state_updates(&drain(&rx));
        assert!(
            ups.iter()
                .any(|u| u.path == "/script/lp/selected" && u.v == 2.0),
            "Int script path mirrored: {ups:?}"
        );
        assert!(
            ups.iter()
                .any(|u| u.path == "/script/lp/mode_n" && u.v == 1.0),
            "numeric mode twin mirrored: {ups:?}"
        );
        assert!(
            ups.iter()
                .any(|u| u.path == "/script/lp/shift" && u.v == 0.0),
            "Bool script path mirrored as 0/1: {ups:?}"
        );
        assert!(
            ups.iter().all(|u| u.path != "/script/lp/mode"),
            "Text script path must never appear in the mirror: {ups:?}"
        );
    }

    #[test]
    fn context_snapshot_shape() {
        let (mut h, rx) = test_handle();
        let mut bus = StateBusHandle::new();
        bus.write("/context/encoder_0/node", StateBusValue::Int(20));
        bus.write(
            "/context/encoder_0/param",
            StateBusValue::Text("cutoff".into()),
        );
        bus.write("/context/encoder_3/node", StateBusValue::Int(21));
        bus.write(
            "/context/encoder_3/param",
            StateBusValue::Text("decay".into()),
        );
        h.pump(&bus, 0);
        let msgs = drain(&rx);
        let ctx = msgs
            .iter()
            .find_map(|m| match m {
                ServerMsg::Context { slots } => Some(slots.clone()),
                _ => None,
            })
            .expect("a context frame");
        assert_eq!(ctx.len(), 2, "two resolved slots, got {ctx:?}");
        assert_eq!(
            ctx[0],
            protocol::ContextSlot {
                enc: 0,
                node: 20,
                param: "cutoff".into()
            }
        );
        assert_eq!(
            ctx[1],
            protocol::ContextSlot {
                enc: 3,
                node: 21,
                param: "decay".into()
            }
        );
    }

    #[test]
    fn context_change_during_non_due_pump_is_not_lost() {
        // Regression: context_dirty must persist across pumps. A context
        // change during a non-due pump updates the shadow; if the dirty flag
        // were per-pump it would be lost and never flushed.
        let (mut h, rx) = test_handle();
        let mut bus = StateBusHandle::new();
        bus.write("/transport/bpm", StateBusValue::Float(120.0));
        h.pump(&bus, 0); // initial flush, no context
        let _ = drain(&rx);

        // Context appears during a non-due pump (within the window).
        bus.write("/context/encoder_1/node", StateBusValue::Int(22));
        bus.write(
            "/context/encoder_1/param",
            StateBusValue::Text("tune".into()),
        );
        h.pump(&bus, 10);
        assert!(drain(&rx).is_empty(), "not due yet — nothing flushed");

        // Next due pump must still emit the context that changed earlier.
        h.pump(&bus, 40);
        let msgs = drain(&rx);
        assert!(
            msgs.iter()
                .any(|m| matches!(m, ServerMsg::Context { slots } if !slots.is_empty())),
            "context change from the non-due pump must still be flushed"
        );
    }
}

#[cfg(test)]
mod semantic_channel_tests {
    use super::*;

    /// Simulates what client threads do: send resolved `NodeCommand`s into
    /// the mpsc channel the handle owns the receiving end of, then drain via
    /// the public API in FIFO order. Exercises the same channel wiring as
    /// `AntiphonServer::spawn` without needing a full client-thread harness.
    #[test]
    fn drain_commands_returns_sent_commands_in_order() {
        let (tx, cmd_rx) = mpsc::channel::<NodeCommand>();
        let mut table = ClientTable::new();
        let (out_tx, _out_rx) = mpsc::channel();
        table.allocate(out_tx).expect("one slot");
        let handle = AntiphonHandle {
            url: String::new(),
            clients: Arc::new(Mutex::new(table)),
            state_shadow: HashMap::new(),
            pending: HashMap::new(),
            context_shadow: HashMap::new(),
            last_flush_ms: None,
            context_dirty: false,
            cmd_rx,
        };

        tx.send(NodeCommand {
            target_id: 20,
            type_id: paraclete_node_api::CMD_SET_PARAM,
            arg0: 123,
            arg1: 0.4,
        })
        .unwrap();
        tx.send(NodeCommand {
            target_id: 21,
            type_id: paraclete_node_api::CMD_BUMP_PARAM,
            arg0: 5,
            arg1: -0.1,
        })
        .unwrap();

        let drained: Vec<NodeCommand> = handle.drain_commands().collect();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].target_id, 20);
        assert_eq!(drained[0].arg0, 123);
        assert_eq!(drained[1].target_id, 21);
        assert_eq!(drained[1].arg0, 5);
    }
}
