// SPDX-License-Identifier: GPL-3.0-or-later
// Shared client-side knowledge of the launchpad.rhai profile surface
// (Theoria legibility phase, design/sessions/s1.md F2/F8/F9).
//
// The profile keeps mode + selection server-side and speaks the pad wire
// vocabulary; this module is the single place the web client encodes that
// contract: which pad ids mean SHIFT/SEQ-EDIT, how a track is selected, and
// which mirrored `/script/lp/*` paths expose the profile's state. When the
// W2 view-plugin API (ADR-032) lands, this hardcoding is what it replaces.

import type { Connection, NodeSummary } from "@paraclete/core";

/** Scene button the profile treats as SHIFT (held + pad = track select). */
export const SHIFT_PAD_ID = 65;
/** Scene button the profile treats as the trigger/sequence mode toggle. */
export const SEQ_EDIT_PAD_ID = 64;

/** Mirrored profile state paths (numeric mirror; see launchpad.rhai). */
export const PATH_SELECTED = "/script/lp/selected";
/** 0 = trigger, 1 = sequence — numeric twin of the Text `/script/lp/mode`. */
export const PATH_MODE_N = "/script/lp/mode_n";

/** The profile's SHIFT+pad gesture only reaches pads 0-7. */
export const MAX_SELECTABLE_TRACKS = 8;

export interface TrackInfo {
  /** 0-based track index (order of sequencer nodes in the welcome snapshot). */
  index: number;
  /** Human name from the instrument file ("Kick"), via NodeSummary.name. */
  name: string;
  /** Sequencer node id (for future per-track state reads). */
  nodeId: number;
}

/** Tracks = sequencer nodes in welcome/topology order (s1.md F5: the track
 * count comes from the actual graph, never a hardcoded constant). */
export function tracksFromNodes(nodes: NodeSummary[]): TrackInfo[] {
  return nodes
    .filter((n) => n.type_tag.startsWith("sequencer"))
    .map((n, index) => ({ index, name: n.name, nodeId: n.id }));
}

/** Select a track: reproduce SHIFT-held + pad-press exactly as a physical or
 * emulated pad sequence would arrive at the gateway, so one profile handles
 * either input source identically. Side effect (profile): enters sequence
 * mode for that track. */
export function sendTrackSelect(conn: Connection, track: number): void {
  if (track < 0 || track >= MAX_SELECTABLE_TRACKS) return;
  conn.send({ t: "pad_down", id: SHIFT_PAD_ID, vel: 65535 });
  conn.send({ t: "pad_down", id: track, vel: 65535 });
  conn.send({ t: "pad_up", id: track });
  conn.send({ t: "pad_up", id: SHIFT_PAD_ID });
}

/** Press SEQ-EDIT: the profile toggles trigger <-> sequence mode. */
export function sendModeToggle(conn: Connection): void {
  conn.send({ t: "pad_down", id: SEQ_EDIT_PAD_ID, vel: 65535 });
  conn.send({ t: "pad_up", id: SEQ_EDIT_PAD_ID });
}
