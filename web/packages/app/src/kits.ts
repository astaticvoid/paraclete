// SPDX-License-Identifier: GPL-3.0-or-later
// Kit-list parsing for the KIT tab (P11 C7). The server publishes the same
// raw bus lines the Theotokos KIT screen parses
// (crates/paraclete-theotokos/src/model.rs):
//
//   /context/kits        Text  `idx:name;idx:name;...`  (non-empty slots
//                          only, in slot order — empty slots are omitted)
//   /context/kit_binding Text  `slot:kit;slot:-1;...`   (all 8 slots,
//                          -1 = unbound)
//
// Both arrive as `/context/*` path slots in the `context` snapshot, and the
// `list_kits` query reply carries the raw `/context/kits` line verbatim.
// Parsers here are pure + tolerant: a transiently half-written line yields
// partial data instead of throwing.

/** Kit store capacity (P11 C2a: 64 slots). */
export const KIT_SLOT_COUNT = 64;
/** Pattern binding slots (P11 C2b: `[Option<KitId>; 8]`, slot-indexed). */
export const BINDING_SLOT_COUNT = 8;

export interface KitEntry {
  /** 0-based kit-store slot index. */
  slot: number;
  name: string;
}

/** Parse a `/context/kits` line (`0:Kick Basic;1:Snare Tight`) into one
 * entry per non-empty kit in slot order. Malformed parts (no `:`, bad slot
 * number, blank name) are skipped; duplicate slots keep the last entry. */
export function parseKitLine(line: string): KitEntry[] {
  const out: KitEntry[] = [];
  const bySlot = new Map<number, string>();
  for (const part of line.split(";")) {
    const trimmed = part.trim();
    if (trimmed.length === 0) continue;
    const colon = trimmed.indexOf(":");
    if (colon <= 0) continue;
    const slot = Number(trimmed.slice(0, colon));
    const name = trimmed.slice(colon + 1).trim();
    if (!Number.isInteger(slot) || slot < 0 || name.length === 0) continue;
    bySlot.set(slot, name);
  }
  for (const [slot, name] of bySlot) out.push({ slot, name });
  out.sort((a, b) => a.slot - b.slot);
  return out;
}

/** Parse a `/context/kit_binding` line (`0:2;1:-1;...`) into exactly
 * `BINDING_SLOT_COUNT` entries; `null` = unbound (a negative or absent kit
 * id, or a line that never mentioned the slot). */
export function parseKitBinding(line: string): (number | null)[] {
  const out: (number | null)[] = new Array<number | null>(BINDING_SLOT_COUNT).fill(null);
  for (const part of line.split(";")) {
    const trimmed = part.trim();
    if (trimmed.length === 0) continue;
    const colon = trimmed.indexOf(":");
    if (colon <= 0) continue;
    const slot = Number(trimmed.slice(0, colon));
    const kit = Number(trimmed.slice(colon + 1));
    if (!Number.isInteger(slot) || slot < 0 || slot >= BINDING_SLOT_COUNT) continue;
    out[slot] = Number.isInteger(kit) && kit >= 0 ? kit : null;
  }
  return out;
}

/** Default save name (`Kit N`): N is the first free slot (1-based), or one
 * past the last occupied slot when the store is full. */
export function defaultKitName(kits: KitEntry[]): string {
  const occupied = new Set(kits.map((k) => k.slot));
  for (let slot = 0; slot < KIT_SLOT_COUNT; slot++) {
    if (!occupied.has(slot)) return `Kit ${slot + 1}`;
  }
  return `Kit ${KIT_SLOT_COUNT + 1}`;
}
