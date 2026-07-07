// SPDX-License-Identifier: GPL-3.0-or-later
// Grid view — ported from the W0 vanilla client (web/theoria/theoria.js,
// deleted in this commit). Same surface layout as the Launchpad/emulator
// control-id map: grid 0-63 (row*8+col), scene 64-71 (right column),
// control row 72-79 (top row). LED batches recolour cells; pointer events
// emit pad_down/pad_up with per-pointer tracking so a drag off a cell
// releases it.

import { useEffect, useRef } from "preact/hooks";
import type { Connection, LedMsgEntry, MessageBus } from "@paraclete/core";

interface Layout {
  cell: number;
  gap: number;
  ox: number;
  oy: number;
}

interface Pos {
  col: number;
  row: number;
}

function cellPos(id: number): Pos | null {
  if (id < 64) return { col: id % 8, row: 1 + Math.floor(id / 8) };
  if (id < 72) return { col: 8, row: 1 + (id - 64) };
  if (id < 80) return { col: id - 72, row: 0 };
  return null;
}

export interface GridProps {
  connection: Connection | null;
  bus: MessageBus;
  /** 0-100; scales `pad_down` velocity (WQ-2). */
  velocityPct: number;
}

export function Grid({ connection, bus, velocityPct }: GridProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const velocityRef = useRef(velocityPct);
  velocityRef.current = velocityPct;
  const connectionRef = useRef(connection);
  connectionRef.current = connection;

  useEffect(() => {
    const canvas = canvasRef.current;
    const container = containerRef.current;
    if (!canvas || !container) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const ledState = new Map<number, [number, number, number]>();
    const activePointers = new Map<number, number>();
    let layout: Layout = { cell: 40, gap: 6, ox: 0, oy: 0 };

    function computeLayout() {
      if (!canvas || !container) return;
      const vw = container.clientWidth;
      const vh = container.clientHeight;
      const dpr = window.devicePixelRatio || 1;
      canvas.width = vw * dpr;
      canvas.height = vh * dpr;
      canvas.style.width = `${vw}px`;
      canvas.style.height = `${vh}px`;
      ctx!.setTransform(dpr, 0, 0, dpr, 0, 0);

      const cols = 9;
      const rows = 9;
      const cell = Math.max(28, Math.floor(Math.min(vw / (cols + 1), vh / (rows + 1))) - 6);
      const gap = Math.max(3, Math.floor(cell * 0.12));
      const w = cols * cell + (cols - 1) * gap;
      const h = rows * cell + (rows - 1) * gap;
      layout = { cell, gap, ox: (vw - w) / 2, oy: (vh - h) / 2 };
      draw();
    }

    function hitTest(x: number, y: number): number | null {
      const { cell, gap, ox, oy } = layout;
      const pitch = cell + gap;
      const col = Math.floor((x - ox) / pitch);
      const row = Math.floor((y - oy) / pitch);
      if (col < 0 || col > 8 || row < 0 || row > 8) return null;
      if (x - ox - col * pitch > cell || y - oy - row * pitch > cell) return null;
      if (row === 0) return col < 8 ? 72 + col : null;
      if (col === 8) return 64 + (row - 1);
      return (row - 1) * 8 + col;
    }

    function draw() {
      if (!ctx || !canvas) return;
      const { cell, gap, ox, oy } = layout;
      const dpr = window.devicePixelRatio || 1;
      const vw = canvas.width / dpr;
      const vh = canvas.height / dpr;
      ctx.clearRect(0, 0, vw, vh);
      const pitch = cell + gap;
      const heldIds = new Set(activePointers.values());
      for (let id = 0; id < 80; id++) {
        const pos = cellPos(id);
        if (!pos) continue;
        const x = ox + pos.col * pitch;
        const y = oy + pos.row * pitch;
        const rgb = ledState.get(id);
        const lit = !!rgb && (rgb[0] | rgb[1] | rgb[2]) > 0;
        const held = heldIds.has(id);
        ctx.fillStyle = lit
          ? `rgb(${rgb![0]},${rgb![1]},${rgb![2]})`
          : id < 64
            ? "#1c1c22"
            : "#15151a";
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

    const unsubscribe = bus.subscribe((msg) => {
      if (msg.t === "led") {
        for (const u of msg.updates as LedMsgEntry[]) {
          ledState.set(u.id, u.rgb);
        }
        draw();
      }
    });

    function velocity16(): number {
      return Math.round((velocityRef.current / 100) * 65535);
    }

    function onPointerDown(e: PointerEvent) {
      const rect = canvas!.getBoundingClientRect();
      const id = hitTest(e.clientX - rect.left, e.clientY - rect.top);
      if (id === null) return;
      activePointers.set(e.pointerId, id);
      connectionRef.current?.send({ t: "pad_down", id, vel: velocity16() });
      draw();
    }

    function releasePointer(e: PointerEvent) {
      const id = activePointers.get(e.pointerId);
      if (id === undefined) return;
      activePointers.delete(e.pointerId);
      connectionRef.current?.send({ t: "pad_up", id });
      draw();
    }

    function onPointerMove(e: PointerEvent) {
      const held = activePointers.get(e.pointerId);
      if (held === undefined) return;
      const rect = canvas!.getBoundingClientRect();
      if (hitTest(e.clientX - rect.left, e.clientY - rect.top) !== held) releasePointer(e);
    }

    canvas.addEventListener("pointerdown", onPointerDown);
    canvas.addEventListener("pointermove", onPointerMove);
    canvas.addEventListener("pointerup", releasePointer);
    canvas.addEventListener("pointercancel", releasePointer);
    canvas.addEventListener("pointerleave", releasePointer);

    const resizeObserver = new ResizeObserver(computeLayout);
    resizeObserver.observe(container);
    computeLayout();

    return () => {
      unsubscribe();
      resizeObserver.disconnect();
      canvas.removeEventListener("pointerdown", onPointerDown);
      canvas.removeEventListener("pointermove", onPointerMove);
      canvas.removeEventListener("pointerup", releasePointer);
      canvas.removeEventListener("pointercancel", releasePointer);
      canvas.removeEventListener("pointerleave", releasePointer);
    };
    // Re-run only if the bus identity changes; connection/velocity are read
    // via refs so live drag/press handlers always see the latest value.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [bus]);

  return (
    <div ref={containerRef} style={{ width: "100%", height: "100%" }}>
      <canvas ref={canvasRef} />
    </div>
  );
}
