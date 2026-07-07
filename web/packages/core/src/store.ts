// SPDX-License-Identifier: GPL-3.0-or-later
// State mirror + context stores. Plain data holders (no DOM); views
// subscribe and re-render on change. `StateStore` mirrors kerygma's numeric
// `state` batches (`/node/{id}/param/{name}`, `/node/{id}/state/{key}`,
// `/transport/*`); `ContextStore` mirrors the full `context` snapshot
// (encoder id -> node/param it currently drives), the same data the TUI
// encoder row reads.

import type { ContextSlot, StateUpdate } from "./protocol";

type Listener = () => void;

/** A path last-changed timestamp lets views flash on external change
 * (sequencer p-lock) without the store knowing anything about rendering. */
export class StateStore {
  private values = new Map<string, number>();
  private changedAtMs = new Map<string, number>();
  private listeners = new Set<Listener>();

  applyUpdates(updates: StateUpdate[]): void {
    if (updates.length === 0) return;
    const t = nowMs();
    for (const u of updates) {
      this.values.set(u.path, u.v);
      this.changedAtMs.set(u.path, t);
    }
    this.notify();
  }

  get(path: string): number | undefined {
    return this.values.get(path);
  }

  /** Milliseconds since this path last changed, or `null` if never seen. */
  msSinceChange(path: string): number | null {
    const t = this.changedAtMs.get(path);
    return t === undefined ? null : nowMs() - t;
  }

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private notify(): void {
    for (const l of this.listeners) l();
  }
}

export class ContextStore {
  private slots = new Map<number, ContextSlot>();
  private listeners = new Set<Listener>();

  applySnapshot(slots: ContextSlot[]): void {
    this.slots.clear();
    for (const s of slots) this.slots.set(s.enc, s);
    this.notify();
  }

  get(enc: number): ContextSlot | undefined {
    return this.slots.get(enc);
  }

  all(): ContextSlot[] {
    return [...this.slots.values()];
  }

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private notify(): void {
    for (const l of this.listeners) l();
  }
}

function nowMs(): number {
  return typeof performance !== "undefined" ? performance.now() : Date.now();
}
