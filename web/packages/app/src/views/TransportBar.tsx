// SPDX-License-Identifier: GPL-3.0-or-later
// Transport bar — play state + BPM (read-only at W1) from `/transport/*`,
// the always-visible mode switch (s1.md F2), connection status, and the
// velocity slider (WQ-2). Track selection moved to the dedicated TrackBar
// (s1.md F8 — it was undiscoverable in here as unlabeled numbered buttons).

import { useEffect, useState } from "preact/hooks";
import type { Connection, StateStore, ConnectionStatus } from "@paraclete/core";
import { PATH_MODE_N, sendModeToggle } from "../profileLink";

export interface TransportBarProps {
  connection: Connection | null;
  stateStore: StateStore;
  status: ConnectionStatus;
  velocityPct: number;
  onVelocityChange: (pct: number) => void;
}

export function TransportBar({
  connection,
  stateStore,
  status,
  velocityPct,
  onVelocityChange,
}: TransportBarProps) {
  const [, setTick] = useState(0);

  useEffect(() => {
    return stateStore.subscribe(() => setTick((t) => t + 1));
  }, [stateStore]);

  const playing = stateStore.get("/transport/playing");
  const bpm = stateStore.get("/transport/bpm");
  // 0 = trigger, 1 = sequence; undefined until the mirror delivers it.
  const modeN = stateStore.get(PATH_MODE_N);

  // Each segment is idempotent: tapping the mode you are already in does
  // nothing; tapping the other one sends the profile's SEQ-EDIT toggle.
  function requestMode(target: 0 | 1) {
    if (!connection || modeN === undefined || modeN === target) return;
    sendModeToggle(connection);
  }

  return (
    <div class="transport-bar">
      <span class={`status ${status}`}>{status}</span>
      <span>{playing === undefined ? "—" : playing > 0 ? "▶ playing" : "■ stopped"}</span>
      <span class="bpm">{bpm !== undefined ? `${bpm.toFixed(1)} BPM` : ""}</span>
      <div class="mode-switch" role="group" aria-label="pad mode">
        <button
          class={`mode-btn ${modeN === 0 ? "active" : ""}`}
          disabled={modeN === undefined}
          onClick={() => requestMode(0)}
        >
          TRIG
        </button>
        <button
          class={`mode-btn ${modeN === 1 ? "active" : ""}`}
          disabled={modeN === undefined}
          onClick={() => requestMode(1)}
        >
          STEP
        </button>
      </div>
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
    </div>
  );
}
