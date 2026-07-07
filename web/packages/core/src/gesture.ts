// SPDX-License-Identifier: GPL-3.0-or-later
// Pointer-drag -> encoder detents. Relative only (w1-interfaces.md
// §Commit 4): the wire never carries an absolute position, only
// `{"t":"enc",id,delta}` detent counts. Coarse and fine are both
// client-side pixel-per-detent scalings of the same drag; the wire shape
// never changes.

export const COARSE_PX_PER_DETENT = 8;
export const FINE_PX_PER_DETENT = 24;

/** Accumulates sub-detent drag distance and yields whole detents as the
 * drag crosses each threshold. One instance per active pointer/encoder. */
export class DragToDetents {
  private accumPx = 0;

  constructor(private pxPerDetent = COARSE_PX_PER_DETENT) {}

  setScale(pxPerDetent: number): void {
    this.pxPerDetent = pxPerDetent;
  }

  /** Feed a raw pointer dy (px, screen-space, +y = down). Dragging UP is the
   * "increase" gesture (matches a physical encoder's up-is-more feel), so
   * this negates dy before accumulating. Returns the signed whole number of
   * detents crossed; the fractional remainder stays accumulated. */
  feed(dy: number): number {
    this.accumPx -= dy;
    const detents = Math.trunc(this.accumPx / this.pxPerDetent);
    this.accumPx -= detents * this.pxPerDetent;
    return detents;
  }

  reset(): void {
    this.accumPx = 0;
  }
}

/** Coalesces per-encoder detents produced within one animation frame into a
 * single accumulated delta, so a fast drag emits one `enc` message per
 * frame instead of one per detent. */
export class FrameCoalescer {
  private pending = new Map<number, number>();

  add(id: number, delta: number): void {
    if (delta === 0) return;
    this.pending.set(id, (this.pending.get(id) ?? 0) + delta);
  }

  /** Drain and clear; call once per animation frame. */
  drain(): Array<{ id: number; delta: number }> {
    if (this.pending.size === 0) return [];
    const out = [...this.pending.entries()].map(([id, delta]) => ({ id, delta }));
    this.pending.clear();
    return out;
  }
}
