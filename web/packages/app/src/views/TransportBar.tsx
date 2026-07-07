// SPDX-License-Identifier: GPL-3.0-or-later
// Transport bar — play state + BPM (read-only at W1) from `/transport/*`,
// track-select buttons, connection status, and the velocity slider (WQ-2).
//
// Track select: the physical/emulated Launchpad profile
// (profiles/launchpad.rhai) selects a track via SHIFT (scene button 65)
// held + a grid pad (0-7) pressed. The web client reproduces the same
// gesture over the existing pad_down/pad_up wire vocabulary rather than
// inventing a new message, so one profile handles either input source
// identically. NOTE: live read-back of `/script/lp/selected` depends on
// antiphon's state-mirror allowlist (currently `/node/*/param|state/*` and
// `/transport/*` only — `/script/*` is intentionally out of scope for this
// commit, see w1-interfaces.md §Commit 2); until that allowlist is
// extended, this view's selection highlight is optimistic (set locally on
// press) rather than server-confirmed. Flagged for paired-session/backend
// follow-up.

import { useEffect, useState } from "preact/hooks";
import type { Connection, MessageBus, StateStore, ConnectionStatus } from "@paraclete/core";

const SHIFT_PAD_ID = 65;
const TRACK_COUNT = 8;

export interface TransportBarProps {
  connection: Connection | null;
  stateStore: StateStore;
  bus: MessageBus;
  status: ConnectionStatus;
  velocityPct: number;
  onVelocityChange: (pct: number) => void;
}

export function TransportBar({
  connection,
  stateStore,
  bus,
  status,
  velocityPct,
  onVelocityChange,
}: TransportBarProps) {
  const [, setTick] = useState(0);
  const [selected, setSelected] = useState<number | null>(null);

  useEffect(() => {
    return stateStore.subscribe(() => setTick((t) => t + 1));
  }, [stateStore]);

  useEffect(() => {
    return bus.subscribe((msg) => {
      // Welcome carries the initial transport snapshot; live updates arrive
      // as /transport/* paths through the state mirror (handled by the
      // stateStore subscription above).
      if (msg.t === "welcome") setTick((t) => t + 1);
    });
  }, [bus]);

  const playing = stateStore.get("/transport/playing");
  const bpm = stateStore.get("/transport/bpm");
  const mirroredSelected = stateStore.get("/script/lp/selected");

  const activeTrack = mirroredSelected ?? selected;

  function selectTrack(t: number) {
    if (!connection) return;
    // Reproduce SHIFT-held + pad-press exactly as a physical/emulated pad
    // sequence would arrive at the gateway.
    connection.send({ t: "pad_down", id: SHIFT_PAD_ID, vel: 65535 });
    connection.send({ t: "pad_down", id: t, vel: 65535 });
    connection.send({ t: "pad_up", id: t });
    connection.send({ t: "pad_up", id: SHIFT_PAD_ID });
    setSelected(t);
  }

  return (
    <div class="transport-bar">
      <span class={`status ${status}`}>{status}</span>
      <span>{playing === undefined ? "—" : playing > 0 ? "▶ playing" : "■ stopped"}</span>
      <span class="bpm">{bpm !== undefined ? `${bpm.toFixed(1)} BPM` : ""}</span>
      <div class="velocity-slider">
        <label htmlFor="velocity">vel</label>
        <input
          id="velocity"
          type="range"
          min={1}
          max={100}
          value={velocityPct}
          onInput={(e) => onVelocityChange(Number((e.target as HTMLInputElement).value))}
        />
        <span>{velocityPct}%</span>
      </div>
      <div class="track-buttons">
        {Array.from({ length: TRACK_COUNT }, (_, t) => (
          <button
            key={t}
            class={`track-btn ${activeTrack === t ? "selected" : ""}`}
            onClick={() => selectTrack(t)}
          >
            {t + 1}
          </button>
        ))}
      </div>
    </div>
  );
}
