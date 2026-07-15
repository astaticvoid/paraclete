// SPDX-License-Identifier: GPL-3.0-or-later
// Track select — vertical left-edge column (W2 Commit 3).
// One button per sequencer track; active track highlighted.
// Tap selects the track and requests view_meta for its engine.

import { useEffect, useState } from "preact/hooks";
import type { Connection, StateStore } from "@paraclete/core";
import {
  MAX_SELECTABLE_TRACKS,
  PATH_SELECTED,
  sendTrackSelect,
  type TrackInfo,
} from "../profileLink";

export interface TrackSelectProps {
  connection: Connection | null;
  stateStore: StateStore;
  tracks: TrackInfo[];
  onTrackSelect?: (index: number) => void;
}

export function TrackSelect({
  connection,
  stateStore,
  tracks,
  onTrackSelect,
}: TrackSelectProps) {
  const [, setTick] = useState(0);
  const [localSelected, setLocalSelected] = useState<number | null>(null);

  useEffect(() => {
    return stateStore.subscribe(() => setTick((t) => t + 1));
  }, [stateStore]);

  const mirrored = stateStore.get(PATH_SELECTED);
  const selected = mirrored !== undefined ? mirrored : localSelected;

  if (tracks.length === 0) {
    return <div class="track-select-col" />;
  }

  return (
    <div class="track-select-col">
      {tracks.map((tr) => (
        <button
          key={tr.nodeId}
          class={`track-col-btn ${selected === tr.index ? "selected" : ""}`}
          disabled={tr.index >= MAX_SELECTABLE_TRACKS}
          onClick={() => {
            if (!connection) return;
            sendTrackSelect(connection, tr.index);
            setLocalSelected(tr.index);
            onTrackSelect?.(tr.index);
          }}
          title={tr.name}
        >
          <span class="track-col-idx">{tr.index + 1}</span>
          <span class="track-col-name">{tr.name}</span>
        </button>
      ))}
    </div>
  );
}
