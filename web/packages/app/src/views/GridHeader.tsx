// SPDX-License-Identifier: GPL-3.0-or-later
// Grid header — one always-visible sentence saying what the pads below do
// right now (s1.md F2/F9): the current mode and, in step-edit mode, which
// track the 16 visible steps belong to.

import { useEffect, useState } from "preact/hooks";
import type { StateStore } from "@paraclete/core";
import { PATH_MODE_N, PATH_SELECTED, type TrackInfo } from "../profileLink";

export interface GridHeaderProps {
  stateStore: StateStore;
  tracks: TrackInfo[];
}

export function GridHeader({ stateStore, tracks }: GridHeaderProps) {
  const [, setTick] = useState(0);

  useEffect(() => {
    return stateStore.subscribe(() => setTick((t) => t + 1));
  }, [stateStore]);

  const modeN = stateStore.get(PATH_MODE_N);
  const selected = stateStore.get(PATH_SELECTED);

  let text: string;
  let modeClass = "";
  if (modeN === undefined) {
    text = "waiting for profile state…";
  } else if (modeN === 1) {
    const track = tracks.find((t) => t.index === selected);
    const name = track ? track.name : selected !== undefined ? `track ${selected + 1}` : "?";
    text = `editing ${name} — pads toggle steps 1–16`;
    modeClass = "step";
  } else {
    text = "pads play tracks — tap a track above to edit its steps";
    modeClass = "trig";
  }

  return (
    <div class={`grid-header ${modeClass}`}>
      {modeN !== undefined && (
        <span class="grid-header-mode">{modeN === 1 ? "STEP" : "TRIG"}</span>
      )}
      <span class="grid-header-text">{text}</span>
    </div>
  );
}
