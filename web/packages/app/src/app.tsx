// SPDX-License-Identifier: GPL-3.0-or-later
// Theoria app shell: owns the one Connection + stores for this tab, wires
// them into the core-stratum views. W2 (ADR-032): adds the editor rail
// layout — left track column, bottom page nav, contextual window.

import { useEffect, useMemo, useState } from "preact/hooks";
import {
  Connection,
  ContextStore,
  MessageBus,
  StateStore,
  type ConnectionStatus,
  type NodeSummary,
  type ServerMsg,
  type ViewMetaMsg,
} from "@paraclete/core";
import { Grid } from "./views/Grid";
import { EncoderRow } from "./views/EncoderRow";
import { TransportBar } from "./views/TransportBar";
import { GridHeader } from "./views/GridHeader";
import { TokenGate } from "./views/TokenGate";
import { TrackSelect } from "./views/TrackSelect";
import { PageNav } from "./views/PageNav";
import { ParamPage } from "./views/ParamPage";
import { ChainView } from "./views/ChainView";
import { tracksFromNodes } from "./profileLink";

const TOKEN_STORAGE_KEY = "antiphon-token";

function wsUrl(): string {
  const wsPort = (Number(location.port) || 80) + 1;
  return `ws://${location.hostname}:${wsPort}/`;
}

export function App() {
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
  const [nodes, setNodes] = useState<NodeSummary[]>([]);
  const tracks = useMemo(() => tracksFromNodes(nodes), [nodes]);

  // W2: editor state
  const [viewMeta, setViewMeta] = useState<ViewMetaMsg | null>(null);
  const [activePage, setActivePage] = useState<string>("GRID");

  useEffect(() => {
    const conn = new Connection({
      url: wsUrl(),
      token,
      client: "theoria-web/0.1",
      onStatus: (s, detail) => {
        setStatus(s);
        if (s === "error" && detail) setErrorText(detail);
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
        } else if (msg.t === "view_meta") {
          setViewMeta(msg);
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

  function handleTrackSelect(index: number) {
    if (!connection) return;
    connection.send({ t: "get_view_meta", track_id: index });
    setActivePage("GRID");
  }

  function handlePageSelect(pageId: string) {
    setActivePage(pageId);
  }

  const pages = viewMeta?.pages ?? [];
  const currentPage = pages.find((p) => p.id === activePage);

  return (
    <div class="app editor-layout">
      {/* Left rail: track select column */}
      <TrackSelect
        connection={connection}
        stateStore={stateStore}
        tracks={tracks}
        onTrackSelect={handleTrackSelect}
      />

      {/* Main area */}
      <div class="main-area">
        {/* Contextual window */}
        <div class="contextual-window">
          {activePage === "GRID" ? (
            <>
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
            </>
          ) : activePage === "CHAIN" && viewMeta ? (
            <ChainView
              connection={connection}
              stateStore={stateStore}
              chain={viewMeta.chain}
              pageIds={pages.map((p) => p.id)}
              onNavigate={handlePageSelect}
            />
          ) : currentPage ? (
            <ParamPage
              connection={connection}
              stateStore={stateStore}
              page={currentPage}
              nodes={nodes}
            />
          ) : (
            <div class="page-placeholder">
              <div class="page-title">{activePage}</div>
              <div class="page-hint">no data for this page</div>
            </div>
          )}
        </div>

        {/* Encoder row */}
        <div class="encoder-row-container">
          <EncoderRow
            connection={connection}
            stateStore={stateStore}
            contextStore={contextStore}
            nodes={nodes}
          />
        </div>

        {/* Bottom rail: page nav + transport */}
        <div class="bottom-rail">
          <PageNav pages={pages} activePage={activePage} onSelect={handlePageSelect} />
          <TransportBar
            connection={connection}
            stateStore={stateStore}
            status={status}
            velocityPct={velocityPct}
            onVelocityChange={setVelocityPct}
          />
        </div>
      </div>

      <div
        class={`overlay-stale ${
          status === "stale" || status === "connecting" ? "visible" : ""
        }`}
      >
        STALE
      </div>
      {tokenRejected ? (
        <TokenGate
          onSubmit={(code) => {
            localStorage.setItem(TOKEN_STORAGE_KEY, code);
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
