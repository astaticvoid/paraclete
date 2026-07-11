// SPDX-License-Identifier: GPL-3.0-or-later
// Track bar — the first-class, labeled track selector (s1.md F8/F9
// keystone). One button per sequencer in the graph, showing its number and
// instrument-file name; the active track is highlighted from the mirrored
// `/script/lp/selected` so selection made on any surface (Launchpad,
// emulator, another tablet) is reflected here. Tapping a track selects it
// and (profile behavior) enters step-edit mode for it.

import { useEffect, useState } from "preact/hooks";
import type { Connection, StateStore } from "@paraclete/core";
import {
  MAX_SELECTABLE_TRACKS,
  PATH_SELECTED,
  sendTrackSelect,
  type TrackInfo,
} from "../profileLink";

export interface TrackBarProps {
  connection: Connection | null;
  stateStore: StateStore;
  tracks: TrackInfo[];
}

export function TrackBar({ connection, stateStore, tracks }: TrackBarProps) {
  const [, setTick] = useState(0);
  // Optimistic highlight for the gap between tap and mirror round-trip.
  const [localSelected, setLocalSelected] = useState<number | null>(null);

  useEffect(() => {
    return stateStore.subscribe(() => setTick((t) => t + 1));
  }, [stateStore]);

  const mirrored = stateStore.get(PATH_SELECTED);
  const selected = mirrored !== undefined ? mirrored : localSelected;

  if (tracks.length === 0) {
    return <div class="track-bar" />;
  }

  // The SHIFT+pad selection gesture only reaches 8 tracks; render any
  // overflow unselectable rather than hiding it (universality note in the
  // phase report — the limit is the wire gesture, not this view).
  return (
    <div class="track-bar">
      <span class="track-bar-label">TRACK</span>
      {tracks.map((tr) => (
        <button
          key={tr.nodeId}
          class={`track-select-btn ${selected === tr.index ? "selected" : ""}`}
          disabled={tr.index >= MAX_SELECTABLE_TRACKS}
          onClick={() => {
            if (!connection) return;
            sendTrackSelect(connection, tr.index);
            setLocalSelected(tr.index);
          }}
        >
          <span class="track-num">{tr.index + 1}</span>
          <span class="track-name">{tr.name}</span>
        </button>
      ))}
    </div>
  );
}
