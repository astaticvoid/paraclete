// SPDX-License-Identifier: GPL-3.0-or-later
// Theoria W0 grid client (zero-build, plain ES module — w0-interfaces.md §Client).
//
// Surface layout matches the Launchpad / emulator control-id map:
//   grid 0–63 (row*8+col, row 0 = top), scene 64–71 (right column),
//   control row 72–79 (top). LED batches recolour cells; pointer events emit
//   pad_down/pad_up with multi-touch tracking so a drag off a cell releases it.

const PROTOCOL = 0;
const STALE_MS = 500;
const PING_INTERVAL_MS = 2000;
const BACKOFF_MS = [1000, 2000, 4000, 5000];

// ── Connection state ──────────────────────────────────────────────────────────

const token = new URLSearchParams(location.search).get("t") ?? "";
// WS listens on HTTP port + 1 (w0-report.md deviation #2).
const wsPort = (Number(location.port) || 80) + 1;
const wsUrl = `ws://${location.hostname}:${wsPort}/`;

let ws = null;
let connected = false;
let reconnectAttempt = 0;
let staleTimer = null;
let hardError = false;

// ── Surface state ─────────────────────────────────────────────────────────────

// Last known colour per control id (0–97); the welcome-time full batch and
// incremental `led` messages land here.
const ledState = new Map();
// pointerId → control id currently held by that finger.
const activePointers = new Map();

// RTT + touch→LED latency instrumentation (WQ-1 measurement).
let lastPongAt = performance.now();
const rttSamples = [];
let latencySamples = [];
// control id → performance.now() of the pad_down awaiting its first LED echo.
const pendingLedEcho = new Map();

// ── Canvas & layout ───────────────────────────────────────────────────────────

const canvas = document.getElementById("surface");
const ctx = canvas.getContext("2d");
const staleEl = document.getElementById("stale");
const errorEl = document.getElementById("error");
const rttEl = document.getElementById("rtt");

// Geometry recomputed on resize: 9 columns (8 grid + scene), 9 rows
// (control row + 8 grid rows), square-ish cells with gaps.
let layout = { cell: 40, gap: 6, ox: 0, oy: 0 };

function computeLayout() {
  const vw = window.visualViewport?.width ?? window.innerWidth;
  const vh = window.visualViewport?.height ?? window.innerHeight;
  const dpr = window.devicePixelRatio || 1;
  canvas.width = vw * dpr;
  canvas.height = vh * dpr;
  canvas.style.width = `${vw}px`;
  canvas.style.height = `${vh}px`;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

  const cols = 9, rows = 9;
  const cell = Math.max(44, Math.floor(Math.min(vw / (cols + 1), vh / (rows + 1))) - 6);
  const gap = Math.max(4, Math.floor(cell * 0.12));
  const w = cols * cell + (cols - 1) * gap;
  const h = rows * cell + (rows - 1) * gap;
  layout = { cell, gap, ox: (vw - w) / 2, oy: (vh - h) / 2 };
  draw();
}

// Control id → {col, row} in the 9×9 canvas lattice (null = empty corner).
function cellPos(id) {
  if (id < 64) return { col: id % 8, row: 1 + Math.floor(id / 8) }; // grid
  if (id < 72) return { col: 8, row: 1 + (id - 64) };               // scene
  if (id < 80) return { col: id - 72, row: 0 };                     // control
  return null;
}

// Canvas point → control id (respecting the empty top-right corner).
function hitTest(x, y) {
  const { cell, gap, ox, oy } = layout;
  const pitch = cell + gap;
  const col = Math.floor((x - ox) / pitch);
  const row = Math.floor((y - oy) / pitch);
  if (col < 0 || col > 8 || row < 0 || row > 8) return null;
  // Inside the cell, not the gap?
  if ((x - ox) - col * pitch > cell || (y - oy) - row * pitch > cell) return null;
  if (row === 0) return col < 8 ? 72 + col : null;      // control row; corner empty
  if (col === 8) return 64 + (row - 1);                  // scene column
  return (row - 1) * 8 + col;                            // grid
}

function draw() {
  const { cell, gap, ox, oy } = layout;
  const vw = canvas.width / (window.devicePixelRatio || 1);
  const vh = canvas.height / (window.devicePixelRatio || 1);
  ctx.clearRect(0, 0, vw, vh);
  const pitch = cell + gap;
  const heldIds = new Set(activePointers.values());
  for (let id = 0; id < 80; id++) {
    const pos = cellPos(id);
    const x = ox + pos.col * pitch;
    const y = oy + pos.row * pitch;
    const rgb = ledState.get(id);
    const lit = rgb && (rgb[0] | rgb[1] | rgb[2]) > 0;
    const held = heldIds.has(id);
    ctx.fillStyle = lit
      ? `rgb(${rgb[0]},${rgb[1]},${rgb[2]})`
      : (id < 64 ? "#1c1c22" : "#15151a");
    ctx.beginPath();
    ctx.roundRect(x, y, cell, cell, Math.floor(cell * 0.18));
    ctx.fill();
    if (held) {
      ctx.strokeStyle = "#eee";
      ctx.lineWidth = 2;
      ctx.stroke();
    }
  }
}

// ── Wire ──────────────────────────────────────────────────────────────────────

function send(obj) {
  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(obj));
  }
}

function showStale() {
  staleEl.style.display = "flex";
}

function connect() {
  if (hardError) return;
  ws = new WebSocket(wsUrl);
  // A dead surface must look dead: STALE within 500 ms of losing the link.
  clearTimeout(staleTimer);
  staleTimer = setTimeout(() => { if (!connected) showStale(); }, STALE_MS);

  ws.addEventListener("open", () => {
    lastPongAt = performance.now();
    send({ t: "hello", token, client: "theoria-web/0.1" });
  });

  ws.addEventListener("message", (e) => {
    let msg;
    try { msg = JSON.parse(e.data); } catch { return; }
    handleMessage(msg);
  });

  ws.addEventListener("close", () => {
    connected = false;
    showStale();
    if (hardError) return;
    const backoff = BACKOFF_MS[Math.min(reconnectAttempt, BACKOFF_MS.length - 1)];
    reconnectAttempt += 1;
    setTimeout(connect, backoff);
  });
  ws.addEventListener("error", () => ws.close());
}

function handleMessage(msg) {
  switch (msg.t) {
    case "welcome":
      if (msg.protocol !== PROTOCOL) {
        hardError = true;
        errorEl.textContent =
          `Protocol mismatch: server speaks v${msg.protocol}, this client v${PROTOCOL}. Reload after updating.`;
        errorEl.style.display = "flex";
        ws.close();
        return;
      }
      connected = true;
      reconnectAttempt = 0;
      clearTimeout(staleTimer);
      staleEl.style.display = "none";
      console.log(`[theoria] welcome: device ${msg.device_id}, ${msg.nodes.length} nodes,` +
        ` transport ${msg.transport.playing ? "playing" : "stopped"} @ ${msg.transport.bpm} BPM`);
      break;
    case "led": {
      const now = performance.now();
      for (const u of msg.updates) {
        ledState.set(u.id, u.rgb);
        const t0 = pendingLedEcho.get(u.id);
        if (t0 !== undefined) {
          pendingLedEcho.delete(u.id);
          const dt = now - t0;
          latencySamples.push(dt);
          if (latencySamples.length > 10) latencySamples.shift();
          updateOverlay();
          console.log(`[theoria] touch→LED ${dt.toFixed(1)} ms (id ${u.id})`);
        }
      }
      draw();
      break;
    }
    case "pong": {
      lastPongAt = performance.now();
      const rtt = performance.now() - msg.ts;
      rttSamples.push(rtt);
      if (rttSamples.length > 10) rttSamples.shift();
      updateOverlay();
      break;
    }
    case "bye":
      console.warn(`[theoria] server said bye: ${msg.reason}`);
      if (msg.reason === "bad token") {
        hardError = true;
        errorEl.textContent = "Bad or missing token — open the exact URL the app printed.";
        errorEl.style.display = "flex";
      }
      // "full": keep the backoff loop running — a slot may free up.
      break;
    default:
      break; // unknown server messages are ignored (forward compatibility)
  }
}

function median(xs) {
  if (xs.length === 0) return null;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.floor(s.length / 2)];
}

function updateOverlay() {
  const rtt = median(rttSamples);
  const lat = median(latencySamples);
  rttEl.textContent =
    (rtt !== null ? `rtt ${rtt.toFixed(1)} ms` : "") +
    (lat !== null ? `  touch→led ${lat.toFixed(1)} ms` : "");
}

setInterval(() => {
  // Two missed pongs → treat the link as dead even without a clean close
  // (tablet Wi-Fi drops silently); close() triggers STALE + reconnect.
  if (connected && performance.now() - lastPongAt > 2.5 * PING_INTERVAL_MS) {
    ws.close();
    return;
  }
  send({ t: "ping", ts: performance.now() });
}, PING_INTERVAL_MS);

// ── Touch input ───────────────────────────────────────────────────────────────

canvas.addEventListener("pointerdown", (e) => {
  const id = hitTest(e.offsetX, e.offsetY);
  if (id === null) return;
  activePointers.set(e.pointerId, id);
  pendingLedEcho.set(id, performance.now());
  send({ t: "pad_down", id, vel: 65535 }); // per-view velocity is W1 (WQ-2)
  draw();
});

function releasePointer(e) {
  const id = activePointers.get(e.pointerId);
  if (id === undefined) return;
  activePointers.delete(e.pointerId);
  send({ t: "pad_up", id });
  draw();
}

// A drag off a cell releases it — same no-stuck-pads guarantee as the
// emulator's held-key map.
canvas.addEventListener("pointermove", (e) => {
  const held = activePointers.get(e.pointerId);
  if (held === undefined) return;
  if (hitTest(e.offsetX, e.offsetY) !== held) releasePointer(e);
});
canvas.addEventListener("pointerup", releasePointer);
canvas.addEventListener("pointercancel", releasePointer);
canvas.addEventListener("pointerleave", releasePointer);

// ── Boot ──────────────────────────────────────────────────────────────────────

window.addEventListener("resize", computeLayout);
window.visualViewport?.addEventListener("resize", computeLayout);
computeLayout();
connect();
