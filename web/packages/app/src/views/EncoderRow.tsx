// SPDX-License-Identifier: GPL-3.0-or-later
// Encoder row — canvas, ids 90-97 (w1-interfaces.md §Commit 4). Vertical
// drag = coarse (1 detent / 8px); dragging in the left/right edge zone of a
// cell, or with a second finger down anywhere on the row, = fine
// (1 detent / 24px). Deltas coalesce per animation frame. Each cell shows
// the param name + live value (state mirror) + a 300 ms flash on external
// change (sequencer p-lock).
//
// Wire: a context-mapped encoder sends the semantic `bump_param` (node,
// param, delta in param units = detents × (max−min)/256, ranges from the
// welcome snapshot) — the server validates against the cap docs and the
// node's ParameterBank clamps the result. An unmapped encoder falls back to
// the raw `{"t":"enc",id,delta}` surface event so profiles can bind it.
// (The W1 spec put the detent→delta scaling in the profile, but profiles
// have no cap-doc access to know a param's range — recorded as a spec
// conflict in the phase report, resolved via the semantic plane the same
// phase shipped.)

import { useEffect, useRef } from "preact/hooks";
import {
  COARSE_PX_PER_DETENT,
  DragToDetents,
  FINE_PX_PER_DETENT,
  FrameCoalescer,
  type Connection,
  type ContextStore,
  type NodeSummary,
  type StateStore,
} from "@paraclete/core";

const ENC_BASE = 90;
const ENC_COUNT = 8;
const FLASH_MS = 300;
const EDGE_ZONE_FRAC = 0.2;
/** Detents to sweep a param's full declared range (w1-interfaces §Commit 4). */
const DETENTS_PER_RANGE = 256;

export interface EncoderRowProps {
  connection: Connection | null;
  stateStore: StateStore;
  contextStore: ContextStore;
  /** Welcome/topology node snapshot — param ranges for delta scaling. */
  nodes: NodeSummary[];
}

export function EncoderRow({ connection, stateStore, contextStore, nodes }: EncoderRowProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const connectionRef = useRef(connection);
  connectionRef.current = connection;
  const nodesRef = useRef(nodes);
  nodesRef.current = nodes;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    let cellW = 0;
    let cellH = 0;
    let dirty = true;

    function resize() {
      if (!canvas) return;
      const parent = canvas.parentElement;
      const vw = parent?.clientWidth ?? canvas.clientWidth;
      const vh = parent?.clientHeight ?? canvas.clientHeight;
      const dpr = window.devicePixelRatio || 1;
      canvas.width = vw * dpr;
      canvas.height = vh * dpr;
      canvas.style.width = `${vw}px`;
      canvas.style.height = `${vh}px`;
      ctx!.setTransform(dpr, 0, 0, dpr, 0, 0);
      cellW = vw / ENC_COUNT;
      cellH = vh;
      dirty = true;
    }

    const dragByPointer = new Map<number, { encIdx: number; drag: DragToDetents; lastY: number }>();
    const coalescer = new FrameCoalescer();

    function encIndexAt(x: number): number {
      return Math.max(0, Math.min(ENC_COUNT - 1, Math.floor(x / cellW)));
    }

    function isEdgeZone(x: number): boolean {
      const localX = x - encIndexAt(x) * cellW;
      return localX < cellW * EDGE_ZONE_FRAC || localX > cellW * (1 - EDGE_ZONE_FRAC);
    }

    function onPointerDown(e: PointerEvent) {
      const rect = canvas!.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const y = e.clientY - rect.top;
      const idx = encIndexAt(x);
      const fine = isEdgeZone(x) || dragByPointer.size >= 1;
      const drag = new DragToDetents(fine ? FINE_PX_PER_DETENT : COARSE_PX_PER_DETENT);
      dragByPointer.set(e.pointerId, { encIdx: idx, drag, lastY: y });
      canvas!.setPointerCapture(e.pointerId);
    }

    function onPointerMove(e: PointerEvent) {
      const st = dragByPointer.get(e.pointerId);
      if (!st) return;
      const rect = canvas!.getBoundingClientRect();
      const y = e.clientY - rect.top;
      const dy = y - st.lastY;
      st.lastY = y;
      const detents = st.drag.feed(dy);
      if (detents !== 0) {
        coalescer.add(ENC_BASE + st.encIdx, detents);
      }
    }

    function onPointerUp(e: PointerEvent) {
      dragByPointer.delete(e.pointerId);
    }

    canvas.addEventListener("pointerdown", onPointerDown);
    canvas.addEventListener("pointermove", onPointerMove);
    canvas.addEventListener("pointerup", onPointerUp);
    canvas.addEventListener("pointercancel", onPointerUp);

    const unsubState = stateStore.subscribe(() => {
      dirty = true;
    });
    const unsubContext = contextStore.subscribe(() => {
      dirty = true;
    });

    // Context slots are keyed by SLOT INDEX 0-7 (see ContextSlot in
    // @paraclete/core), while this view's cells double as surface encoder
    // ids 90-97 for the raw-event fallback.
    function paramPath(slotIdx: number): string | null {
      const slot = contextStore.get(slotIdx);
      if (!slot || !slot.param) return null;
      return `/node/${slot.node}/param/${slot.param}`;
    }

    /** Route one frame's coalesced detents for one encoder to the wire. */
    function sendDetents(encId: number, detents: number) {
      const conn = connectionRef.current;
      if (!conn) return;
      const slot = contextStore.get(encId - ENC_BASE);
      if (slot && slot.param) {
        const param = nodesRef.current
          .find((n) => n.id === slot.node)
          ?.params.find((p) => p.name === slot.param);
        if (param) {
          const delta = detents * ((param.max - param.min) / DETENTS_PER_RANGE);
          conn.send({ t: "bump_param", node: slot.node, param: slot.param, delta });
          return;
        }
      }
      // Unmapped (or stale context): raw surface event for profiles.
      conn.send({ t: "enc", id: encId, delta: detents });
    }

    /** Value formatting scaled to magnitude — "4000" not "4000.000". */
    function formatValue(v: number): string {
      const a = Math.abs(v);
      if (a >= 100) return v.toFixed(0);
      if (a >= 10) return v.toFixed(1);
      if (a >= 1) return v.toFixed(2);
      return v.toFixed(3);
    }

    function draw() {
      if (!ctx || !canvas) return;
      const dpr = window.devicePixelRatio || 1;
      const vw = canvas.width / dpr;
      const vh = canvas.height / dpr;
      ctx.clearRect(0, 0, vw, vh);
      for (let i = 0; i < ENC_COUNT; i++) {
        const x = i * cellW;
        const slot = contextStore.get(i);
        const path = paramPath(i);
        const value = path ? stateStore.get(path) : undefined;
        const msSince = path ? stateStore.msSinceChange(path) : null;
        const flashing = msSince !== null && msSince < FLASH_MS;

        ctx.fillStyle = flashing ? "#2a5" : "#1c1c22";
        ctx.fillRect(x + 3, 3, cellW - 6, vh - 6);
        ctx.strokeStyle = "#33333c";
        ctx.strokeRect(x + 3, 3, cellW - 6, vh - 6);

        ctx.fillStyle = flashing ? "#041" : "#999";
        ctx.font = "600 11px ui-monospace, monospace";
        ctx.textAlign = "center";
        ctx.fillText(slot?.param ? slot.param : "—", x + cellW / 2, vh / 2 - 8, cellW - 10);

        ctx.fillStyle = flashing ? "#041" : "#ddd";
        ctx.font = "500 13px ui-monospace, monospace";
        ctx.fillText(
          value !== undefined ? formatValue(value) : "",
          x + cellW / 2,
          vh / 2 + 12,
          cellW - 10,
        );
      }
    }

    let rafId = 0;
    function frame() {
      const drained = coalescer.drain();
      for (const { id, delta } of drained) {
        sendDetents(id, delta);
      }
      if (dirty || drained.length > 0) {
        draw();
        dirty = false;
      }
      rafId = requestAnimationFrame(frame);
    }
    rafId = requestAnimationFrame(frame);

    const resizeObserver = new ResizeObserver(resize);
    if (canvas.parentElement) resizeObserver.observe(canvas.parentElement);
    resize();

    return () => {
      cancelAnimationFrame(rafId);
      resizeObserver.disconnect();
      unsubState();
      unsubContext();
      canvas.removeEventListener("pointerdown", onPointerDown);
      canvas.removeEventListener("pointermove", onPointerMove);
      canvas.removeEventListener("pointerup", onPointerUp);
      canvas.removeEventListener("pointercancel", onPointerUp);
    };
    // connection is read via a ref so the rAF loop always sees the latest.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stateStore, contextStore]);

  return <canvas ref={canvasRef} />;
}
