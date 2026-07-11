// SPDX-License-Identifier: GPL-3.0-or-later
// Grid view — mode-aware pad surface (s2.md F3 + "hide dead grid").
//
// The pre-s2 view mirrored the full 9×9 Launchpad geometry; on glass that
// meant ~64 dead pads ("chaotic and unfocused — a lot of stuff that shows
// with no util in view"). This view renders ONLY the cells that have a
// function under the active profile mode, sized to fill the area:
//
//   TRIG mode:  one large pad per real track (ids 0..N-1, names printed).
//   STEP mode:  16 large step cells (ids 0-15, 2 rows of 8).
//
// SEQ (64) and SHIFT (65) never render here — the TransportBar mode switch
// and the TrackBar already speak those gestures on glass. The wire is
// unchanged: cells still emit pad_down/pad_up with the same control ids, so
// one profile serves the physical Launchpad and this surface identically.
//
// STEP mode drags PAINT (s2.md F3): the first cell touched decides the
// target state (the opposite of what it was), and every cell the drag
// crosses is set to that state — never blind-toggled, so sweeping across a
// mixed run leaves a solid run. The wire vocabulary is still the profile's
// toggle tap; the client just skips cells already in the target state. Step
// state comes from the profile's `/script/lp/steps_n` mask mirror
// (authoritative), never from LED colors (the playhead hides the step
// beneath it).
//
// TRIG mode keeps momentary semantics: down fires, sliding off releases.

import { useEffect, useRef } from "preact/hooks";
import type { Connection, LedMsgEntry, MessageBus, StateStore } from "@paraclete/core";
import {
  PATH_MODE_N,
  PATH_STEPS_N,
  STEP_CELL_COUNT,
  STEP_COLS,
  type TrackInfo,
} from "../profileLink";

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** One drag in STEP mode: paint every crossed cell to `target`. `painted`
 * is not redundant with the stepActive guard in paintCell: a mask-mirror
 * update arriving mid-drag can overwrite optimistic entries with pre-toggle
 * state (the echo is ~35 ms behind), and without the set a re-crossed cell
 * would then be toggled a second time. */
interface PaintDrag {
  target: boolean;
  painted: Set<number>;
}

export interface GridProps {
  connection: Connection | null;
  bus: MessageBus;
  stateStore: StateStore;
  tracks: TrackInfo[];
  /** 0-100; scales `pad_down` velocity (WQ-2). */
  velocityPct: number;
}

export function Grid({ connection, bus, stateStore, tracks, velocityPct }: GridProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const velocityRef = useRef(velocityPct);
  velocityRef.current = velocityPct;
  const connectionRef = useRef(connection);
  connectionRef.current = connection;
  const tracksRef = useRef(tracks);
  tracksRef.current = tracks;
  const relayoutRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const container = containerRef.current;
    if (!canvas || !container) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const ledState = new Map<number, [number, number, number]>();
    // Momentary holds (TRIG mode), per pointer.
    const heldByPointer = new Map<number, number>();
    // Paint drags (STEP mode), per pointer.
    const paintByPointer = new Map<number, PaintDrag>();
    // Selected track's per-step on/off, from the profile's steps_n mask
    // mirror. Our own paints update entries optimistically ahead of the
    // ~35 ms echo; the next mask update overwrites with the true state.
    const stepActive = new Array<boolean>(STEP_CELL_COUNT).fill(false);
    let stepsMask: number | undefined;

    function refreshStepsFromMask() {
      const m = stateStore.get(PATH_STEPS_N);
      if (m === stepsMask || m === undefined) return;
      stepsMask = m;
      for (let i = 0; i < STEP_CELL_COUNT; i++) {
        stepActive[i] = (m & (1 << i)) !== 0;
      }
    }
    refreshStepsFromMask();
    // 0 = trigger, 1 = sequence, undefined = mirror not delivered yet.
    let modeN: number | undefined = stateStore.get(PATH_MODE_N);
    let cellRects: Rect[] = [];
    let viewW = 0;
    let viewH = 0;

    function visibleCellCount(): number {
      if (modeN === 1) return STEP_CELL_COUNT;
      // Every real track is a playable pad (the wire reaches any pad id;
      // MAX_SELECTABLE_TRACKS caps the SHIFT+pad SELECT gesture, not this).
      if (modeN === 0) return tracksRef.current.length;
      return 0;
    }

    /** Cell id -> rect, for the current mode. TRIG: track pads, wrapping at
     * STEP_COLS per row. STEP: steps in the profile's two banks of 8. Cells
     * grow to fill the area within a comfortable maximum, so nothing dead
     * pads the space; the lower clamp keeps cells hittable (and the layout
     * sane) on absurdly small viewports. */
    function computeCells() {
      const n = visibleCellCount();
      cellRects = [];
      if (n === 0) return;
      const cols = modeN === 1 ? STEP_COLS : Math.min(n, STEP_COLS);
      const rows = Math.ceil(n / cols);
      const maxCell = modeN === 1 ? 150 : 220;
      const cell = Math.max(
        8,
        Math.min(
          maxCell,
          Math.floor(viewW / (cols + (cols + 1) * 0.1)),
          Math.floor(viewH / (rows + (rows + 1) * 0.18)),
        ),
      );
      const gap = Math.max(4, Math.floor(cell * 0.1));
      const w = cols * cell + (cols - 1) * gap;
      const h = rows * cell + (rows - 1) * gap;
      const ox = (viewW - w) / 2;
      const oy = (viewH - h) / 2;
      for (let id = 0; id < n; id++) {
        const col = id % cols;
        const row = Math.floor(id / cols);
        cellRects.push({ x: ox + col * (cell + gap), y: oy + row * (cell + gap), w: cell, h: cell });
      }
    }

    function computeLayout() {
      if (!canvas || !container) return;
      viewW = container.clientWidth;
      viewH = container.clientHeight;
      const dpr = window.devicePixelRatio || 1;
      canvas.width = viewW * dpr;
      canvas.height = viewH * dpr;
      canvas.style.width = `${viewW}px`;
      canvas.style.height = `${viewH}px`;
      ctx!.setTransform(dpr, 0, 0, dpr, 0, 0);
      computeCells();
      draw();
    }

    function hitTest(x: number, y: number): number | null {
      for (let id = 0; id < cellRects.length; id++) {
        const r = cellRects[id];
        if (x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h) return id;
      }
      return null;
    }

    function padLabel(id: number): string | null {
      if (modeN === 1) return String(id + 1);
      if (modeN === 0 && id < tracksRef.current.length) return tracksRef.current[id].name;
      return null;
    }

    function draw() {
      if (!ctx || !canvas) return;
      ctx.clearRect(0, 0, viewW, viewH);
      const heldIds = new Set(heldByPointer.values());

      ctx.textAlign = "center";
      ctx.textBaseline = "middle";

      for (let id = 0; id < cellRects.length; id++) {
        const r = cellRects[id];
        const rgb = ledState.get(id);
        const lit = !!rgb && (rgb[0] | rgb[1] | rgb[2]) > 0;
        ctx.fillStyle = lit ? `rgb(${rgb![0]},${rgb![1]},${rgb![2]})` : "#1c1c22";
        ctx.beginPath();
        ctx.roundRect(r.x, r.y, r.w, r.h, Math.floor(r.w * 0.12));
        ctx.fill();
        if (heldIds.has(id)) {
          ctx.strokeStyle = "#eee";
          ctx.lineWidth = 2;
          ctx.stroke();
        }

        const label = padLabel(id);
        if (label) {
          // Contrast against the pad, not the page: dark text on a bright
          // LED, muted light text on an unlit pad.
          const bright = lit && rgb![0] * 0.35 + rgb![1] * 0.5 + rgb![2] * 0.15 > 90;
          ctx.fillStyle = bright ? "rgba(0,0,0,0.75)" : lit ? "#fff" : "#8b8b96";
          ctx.font = `600 ${Math.max(11, Math.floor(r.w * 0.2))}px ui-monospace, monospace`;
          ctx.fillText(label, r.x + r.w / 2, r.y + r.h / 2 + 0.5, r.w - 8);
        }
      }
    }
    relayoutRef.current = () => {
      computeCells();
      draw();
    };

    const unsubscribe = bus.subscribe((msg) => {
      if (msg.t === "led") {
        for (const u of msg.updates as LedMsgEntry[]) {
          ledState.set(u.id, u.rgb);
        }
        draw();
      }
    });

    // Mode change relays out the cells entirely (track pads <-> step cells);
    // steps_n keeps the paint mirror fresh; everything else is ignored here.
    const unsubState = stateStore.subscribe(() => {
      refreshStepsFromMask();
      const m = stateStore.get(PATH_MODE_N);
      if (m !== modeN) {
        modeN = m;
        // A mode change invalidates any in-flight gesture: its cells no
        // longer mean what they meant when the finger landed. Held TRIG
        // pads get their pad_up (down without up is a stuck note to any
        // consumer that tracks pad state); paint drags are just dropped
        // (each paint was a complete down+up tap).
        for (const id of heldByPointer.values()) {
          connectionRef.current?.send({ t: "pad_up", id });
        }
        heldByPointer.clear();
        paintByPointer.clear();
        computeCells();
        draw();
      }
    });

    function velocity16(): number {
      return Math.round((velocityRef.current / 100) * 65535);
    }

    /** One toggle tap on a step cell, with an optimistic local mirror so a
     * drag crossing the cell again this frame reads the post-tap state. */
    function paintCell(id: number, drag: PaintDrag) {
      drag.painted.add(id);
      if (stepActive[id] === drag.target) return;
      stepActive[id] = drag.target;
      connectionRef.current?.send({ t: "pad_down", id, vel: 65535 });
      connectionRef.current?.send({ t: "pad_up", id });
    }

    function onPointerDown(e: PointerEvent) {
      const rect = canvas!.getBoundingClientRect();
      const id = hitTest(e.clientX - rect.left, e.clientY - rect.top);
      if (id === null) return;
      // Capture keeps the drag alive outside the canvas; losing it (synthetic
      // events, an already-released pointer) must not kill the gesture.
      try {
        canvas!.setPointerCapture(e.pointerId);
      } catch {
        /* uncaptured drag still works while over the canvas */
      }
      if (modeN === 1) {
        const drag: PaintDrag = { target: !stepActive[id], painted: new Set() };
        paintByPointer.set(e.pointerId, drag);
        // First cell always toggles — that toggle IS the target choice.
        paintCell(id, drag);
      } else {
        heldByPointer.set(e.pointerId, id);
        connectionRef.current?.send({ t: "pad_down", id, vel: velocity16() });
      }
      draw();
    }

    function releaseHeld(e: PointerEvent) {
      const id = heldByPointer.get(e.pointerId);
      if (id === undefined) return;
      heldByPointer.delete(e.pointerId);
      connectionRef.current?.send({ t: "pad_up", id });
      draw();
    }

    function onPointerMove(e: PointerEvent) {
      const paint = paintByPointer.get(e.pointerId);
      if (paint) {
        const rect = canvas!.getBoundingClientRect();
        const id = hitTest(e.clientX - rect.left, e.clientY - rect.top);
        if (id !== null && !paint.painted.has(id)) paintCell(id, paint);
        return;
      }
      const held = heldByPointer.get(e.pointerId);
      if (held === undefined) return;
      const rect = canvas!.getBoundingClientRect();
      if (hitTest(e.clientX - rect.left, e.clientY - rect.top) !== held) releaseHeld(e);
    }

    function onPointerUp(e: PointerEvent) {
      paintByPointer.delete(e.pointerId);
      releaseHeld(e);
    }

    canvas.addEventListener("pointerdown", onPointerDown);
    canvas.addEventListener("pointermove", onPointerMove);
    canvas.addEventListener("pointerup", onPointerUp);
    canvas.addEventListener("pointercancel", onPointerUp);
    // Safety net for the capture-failed case only: while capture holds,
    // the browser treats the pointer as over the canvas and never fires
    // leave, so this changes nothing on the normal path. Without capture a
    // finger sliding off the edge would otherwise strand a held TRIG pad
    // with no pad_up (events stop arriving once the pointer is outside).
    canvas.addEventListener("pointerleave", onPointerUp);

    const resizeObserver = new ResizeObserver(computeLayout);
    resizeObserver.observe(container);
    computeLayout();

    return () => {
      relayoutRef.current = null;
      unsubscribe();
      unsubState();
      resizeObserver.disconnect();
      canvas.removeEventListener("pointerdown", onPointerDown);
      canvas.removeEventListener("pointermove", onPointerMove);
      canvas.removeEventListener("pointerup", onPointerUp);
      canvas.removeEventListener("pointercancel", onPointerUp);
      canvas.removeEventListener("pointerleave", onPointerUp);
    };
    // Re-run only if the bus/store identity changes; connection/velocity/
    // tracks are read via refs so handlers always see the latest value.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [bus, stateStore]);

  // Welcome (or a topology patch) delivered new track names: TRIG mode's
  // cell count and labels both come from it.
  useEffect(() => {
    relayoutRef.current?.();
  }, [tracks]);

  return (
    <div ref={containerRef} class="grid-canvas-host">
      <canvas ref={canvasRef} />
    </div>
  );
}
