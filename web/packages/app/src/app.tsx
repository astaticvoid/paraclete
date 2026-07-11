// SPDX-License-Identifier: GPL-3.0-or-later
// Theoria app shell: owns the one Connection + stores for this tab, wires
// them into the four core-stratum views (w1-interfaces.md §Commit 4).
// No view-plugin registry yet — that's W2 (ADR-032).

import { useEffect, useMemo, useState } from "preact/hooks";
import {
  Connection,
  ContextStore,
  MessageBus,
  StateStore,
  type ConnectionStatus,
  type NodeSummary,
  type ServerMsg,
} from "@paraclete/core";
import { Grid } from "./views/Grid";
import { EncoderRow } from "./views/EncoderRow";
import { TransportBar } from "./views/TransportBar";
import { TrackBar } from "./views/TrackBar";
import { GridHeader } from "./views/GridHeader";
import { TokenGate } from "./views/TokenGate";
import { tracksFromNodes } from "./profileLink";

const TOKEN_STORAGE_KEY = "antiphon-token";

// WS listens on HTTP port + 1 (w0-report.md deviation #2, carried forward).
function wsUrl(): string {
  const wsPort = (Number(location.port) || 80) + 1;
  return `ws://${location.hostname}:${wsPort}/`;
}

export function App() {
  // Token resolution: explicit ?t= (desktop click-through) beats the code
  // remembered from a previous TokenGate entry; empty string is valid for
  // --open servers. A rejected token routes back through the gate below.
  const token = useMemo(
    () =>
      new URLSearchParams(location.search).get("t") ??
      localStorage.getItem(TOKEN_STORAGE_KEY) ??
      "",
    [],
  );
  const stateStore = useMemo(() => new StateStore(), []);
  const contextStore = useMemo(() => new ContextStore(), []);
  const bus = useMemo(() => new MessageBus(), []);

  const [status, setStatus] = useState<ConnectionStatus>("connecting");
  const [errorText, setErrorText] = useState<string | null>(null);
  const [rttText, setRttText] = useState("");
  const [velocityPct, setVelocityPct] = useState(100);
  const [connection, setConnection] = useState<Connection | null>(null);
  // Full node snapshot from welcome/topology: the encoder row needs param
  // ranges for bump scaling; tracks (count + names) derive from it — the
  // graph is the source of truth, never a hardcoded constant (F5).
  const [nodes, setNodes] = useState<NodeSummary[]>([]);
  const tracks = useMemo(() => tracksFromNodes(nodes), [nodes]);

  useEffect(() => {
    const conn = new Connection({
      url: wsUrl(),
      token,
      client: "theoria-web/0.1",
      onStatus: (s, detail) => {
        setStatus(s);
        if (s === "error" && detail) setErrorText(detail);
        // Handshake accepted: remember the code so the bare URL works on
        // every future visit from this device.
        if (s === "open" && token) {
          localStorage.setItem(TOKEN_STORAGE_KEY, token);
        }
      },
      onMessage: (msg: ServerMsg) => {
        if (msg.t === "state") {
          stateStore.applyUpdates(msg.updates);
        } else if (msg.t === "context") {
          contextStore.applySnapshot(msg.slots);
        } else if (msg.t === "welcome" || msg.t === "topology") {
          setNodes(msg.nodes);
        }
        bus.emit(msg);
      },
    });
    setConnection(conn);
    conn.connect();

    const rttTimer = setInterval(() => {
      const rtt = conn.medianRttMs();
      setRttText(rtt !== null ? `rtt ${rtt.toFixed(1)} ms` : "");
    }, 1000);

    return () => clearInterval(rttTimer);
  }, [token, stateStore, contextStore, bus]);

  const tokenRejected = status === "error" && (errorText ?? "").includes("token");

  return (
    <div class="app">
      <TransportBar
        connection={connection}
        stateStore={stateStore}
        status={status}
        velocityPct={velocityPct}
        onVelocityChange={setVelocityPct}
      />
      <TrackBar connection={connection} stateStore={stateStore} tracks={tracks} />
      <div class="encoder-row-container">
        <EncoderRow
          connection={connection}
          stateStore={stateStore}
          contextStore={contextStore}
          nodes={nodes}
        />
      </div>
      <GridHeader stateStore={stateStore} tracks={tracks} />
      <div class="grid-container">
        <Grid
          connection={connection}
          bus={bus}
          stateStore={stateStore}
          tracks={tracks}
          velocityPct={velocityPct}
        />
      </div>
      <div class={`overlay-stale ${status === "stale" || status === "connecting" ? "visible" : ""}`}>
        STALE
      </div>
      {tokenRejected ? (
        <TokenGate
          onSubmit={(code) => {
            localStorage.setItem(TOKEN_STORAGE_KEY, code);
            // Full reload with a clean URL: the connection is rebuilt with
            // the stored code (wrong code -> gate again, retype).
            location.replace(location.pathname);
          }}
        />
      ) : (
        <div class={`overlay-error ${status === "error" ? "visible" : ""}`}>
          {errorText ?? "Connection error — reload after updating."}
        </div>
      )}
      <div class="rtt-readout">{rttText}</div>
    </div>
  );
}
