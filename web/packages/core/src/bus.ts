// SPDX-License-Identifier: GPL-3.0-or-later
// A tiny fan-out for raw `ServerMsg`s. `StateStore`/`ContextStore` consume
// `state`/`context` for their own bookkeeping; other views (grid LEDs,
// welcome/topology, transport) subscribe here directly rather than every
// view owning its own WebSocket listener.

import type { ServerMsg } from "./protocol";

export class MessageBus {
  private listeners = new Set<(msg: ServerMsg) => void>();

  emit(msg: ServerMsg): void {
    for (const l of this.listeners) l(msg);
  }

  subscribe(fn: (msg: ServerMsg) => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }
}
