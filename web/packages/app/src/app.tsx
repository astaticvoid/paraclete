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
  type ServerMsg,
} from "@paraclete/core";
import { Grid } from "./views/Grid";
import { EncoderRow } from "./views/EncoderRow";
import { TransportBar } from "./views/TransportBar";

// WS listens on HTTP port + 1 (w0-report.md deviation #2, carried forward).
function wsUrl(): string {
  const wsPort = (Number(location.port) || 80) + 1;
  return `ws://${location.hostname}:${wsPort}/`;
}

export function App() {
  const token = useMemo(() => new URLSearchParams(location.search).get("t") ?? "", []);
  const stateStore = useMemo(() => new StateStore(), []);
  const contextStore = useMemo(() => new ContextStore(), []);
  const bus = useMemo(() => new MessageBus(), []);

  const [status, setStatus] = useState<ConnectionStatus>("connecting");
  const [errorText, setErrorText] = useState<string | null>(null);
  const [rttText, setRttText] = useState("");
  const [velocityPct, setVelocityPct] = useState(100);
  const [connection, setConnection] = useState<Connection | null>(null);

  useEffect(() => {
    const conn = new Connection({
      url: wsUrl(),
      token,
      client: "theoria-web/0.1",
      onStatus: (s, detail) => {
        setStatus(s);
        if (s === "error" && detail) setErrorText(detail);
      },
      onMessage: (msg: ServerMsg) => {
        if (msg.t === "state") {
          stateStore.applyUpdates(msg.updates);
        } else if (msg.t === "context") {
          contextStore.applySnapshot(msg.slots);
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

  return (
    <div class="app">
      <TransportBar
        connection={connection}
        stateStore={stateStore}
        bus={bus}
        status={status}
        velocityPct={velocityPct}
        onVelocityChange={setVelocityPct}
      />
      <div class="encoder-row-container">
        <EncoderRow connection={connection} stateStore={stateStore} contextStore={contextStore} />
      </div>
      <div class="grid-container">
        <Grid connection={connection} bus={bus} velocityPct={velocityPct} />
      </div>
      <div class={`overlay-stale ${status === "stale" || status === "connecting" ? "visible" : ""}`}>
        STALE
      </div>
      <div class={`overlay-error ${status === "error" ? "visible" : ""}`}>
        {errorText ?? "Connection error — reload after updating."}
      </div>
      <div class="rtt-readout">{rttText}</div>
    </div>
  );
}
