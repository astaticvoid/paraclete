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

use std::net::UdpSocket;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kerygma::{ClientTable, Kerygma, MAX_CLIENTS};
use protocol::{NodeSummary, TransportSummary};
use server::SessionInfo;
use surface::TheoriaSurfaceNode;

/// Default HTTP port (WebSocket listens on `port + 1`).
/// 7274 = "PARA" on a phone keypad.
pub const DEFAULT_PORT: u16 = 7274;

pub struct AntiphonConfig {
    /// HTTP port for the static client bundle; the WS server binds `port + 1`.
    pub port: u16,
    /// Session token; gates the WS handshake. The app generates and prints it.
    pub token: String,
    /// Directory of the Theoria client bundle; `None` disables the HTTP side
    /// (WS-only — useful for tests and headless drivers).
    pub static_dir: Option<PathBuf>,
    /// Node id the app will register the surface under (`welcome.device_id`).
    pub device_id: u32,
}

/// Main-thread handle to the running server. W1 extends this with the state
/// mirror pump and topology publishing.
pub struct AntiphonHandle {
    /// Printable client URL, token included.
    pub url: String,
    clients: Arc<Mutex<ClientTable>>,
}

impl AntiphonHandle {
    pub fn client_count(&self) -> usize {
        self.clients.lock().map(|t| t.active_count()).unwrap_or(0)
    }
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
    ) -> std::io::Result<(TheoriaSurfaceNode, AntiphonHandle)> {
        let clients = Arc::new(Mutex::new(ClientTable::new()));
        let (node, producers) =
            TheoriaSurfaceNode::new(Kerygma::new(Arc::clone(&clients)));
        debug_assert_eq!(producers.len(), MAX_CLIENTS);
        let pool: server::ProducerPool =
            Arc::new(Mutex::new(producers.into_iter().map(Some).collect()));

        let session = Arc::new(SessionInfo {
            token: config.token.clone(),
            device_id: config.device_id,
            nodes,
            transport,
        });
        server::spawn_listener(config.port + 1, session, Arc::clone(&clients), pool)?;
        if let Some(dir) = config.static_dir {
            http::spawn_http(dir, config.port)?;
        }

        let url = format!("http://{}:{}/?t={}", lan_ip(), config.port, config.token);
        Ok((node, AntiphonHandle { url, clients }))
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
            TransportSummary { playing: false, bpm: 120.0 },
        )
        .expect("spawn");
        let mut out_handle =
            paraclete_node_api::Surface::take_output_handle(&mut node).expect("handle");

        let stream = TcpStream::connect(("127.0.0.1", port + 1)).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let (mut ws, _) =
            tungstenite::client("ws://127.0.0.1:47275/", stream).expect("ws handshake");

        // hello → welcome
        let hello = serde_json::to_string(&ClientMsg::Hello {
            token: "cafebabe".into(),
            client: "smoke/0".into(),
        })
        .unwrap();
        ws.send(Message::Text(hello)).unwrap();
        let Message::Text(frame) = ws.read().unwrap() else { panic!("expected text") };
        let ServerMsg::Welcome { protocol, device_id, .. } =
            serde_json::from_str(&frame).unwrap()
        else {
            panic!("expected welcome, got {frame}")
        };
        assert_eq!(protocol, 0);
        assert_eq!(device_id, 106);
        assert_eq!(handle.client_count(), 1);

        // Full-surface replay arrives once the main-thread handle ticks.
        out_handle.tick();
        let Message::Text(frame) = ws.read().unwrap() else { panic!("expected text") };
        assert!(matches!(
            serde_json::from_str::<ServerMsg>(&frame).unwrap(),
            ServerMsg::Led { .. }
        ));

        // ping → pong
        ws.send(Message::Text(r#"{"t":"ping","ts":7.5}"#.into())).unwrap();
        let Message::Text(frame) = ws.read().unwrap() else { panic!("expected text") };
        assert_eq!(
            serde_json::from_str::<ServerMsg>(&frame).unwrap(),
            ServerMsg::Pong { ts: 7.5 }
        );

        ws.close(None).ok();
    }
}
