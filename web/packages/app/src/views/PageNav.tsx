// SPDX-License-Identifier: GPL-3.0-or-later
// PageNav — bottom-edge page navigation row (W2 Commit 3).
// TRIG · SRC · FLTR · AMP · FX · MOD — the Elektron page keys.
// Each button lights when that page has content in the active track's
// view_meta; the active page is highlighted. Empty pages are dimmed.
// KIT is a fixed tab like GRID/CHAIN (not a view_meta page): the P11 C7
// kit list, load/save/commit/reload, and pattern bindings.

import type { ViewMetaPage } from "@paraclete/core";

export interface PageNavProps {
  pages: ViewMetaPage[];
  activePage: string | null;
  onSelect: (pageId: string) => void;
}

const PAGE_LABELS: Record<string, string> = {
  TRIG: "TRIG",
  SRC: "SRC",
  FLTR: "FLTR",
  AMP: "AMP",
  FX: "FX",
  MOD: "MOD",
  GRID: "GRID",
  CHAIN: "CHAIN",
  KIT: "KIT",
};

const PAGE_ORDER = ["GRID", "TRIG", "SRC", "FLTR", "AMP", "FX", "MOD", "CHAIN", "KIT"];

export function PageNav({ pages, activePage, onSelect }: PageNavProps) {
  const pageIds = new Set(pages.map((p) => p.id));

  // Collect custom engine-declared pages beyond the default set
  const customIds = pages
    .map((p) => p.id)
    .filter((id) => !PAGE_ORDER.includes(id));

  return (
    <div class="page-nav">
      {PAGE_ORDER.map((id) => {
        const hasContent = pageIds.has(id) || id === "GRID" || id === "CHAIN" || id === "KIT";
        const label = PAGE_LABELS[id] ?? id;
        return (
          <button
            key={id}
            class={`page-nav-btn ${id === activePage ? "active" : ""} ${
              !hasContent ? "dimmed" : ""
            }`}
            disabled={!hasContent}
            onClick={() => onSelect(id)}
            aria-label={`${label} page`}
          >
            {label}
          </button>
        );
      })}
      {customIds.map((id) => {
        const label = id.length > 4 ? id.slice(0, 4) : id;
        return (
          <button
            key={id}
            class={`page-nav-btn ${id === activePage ? "active" : ""}`}
            onClick={() => onSelect(id)}
            aria-label={`${id} page`}
          >
            {label}
          </button>
        );
      })}
    </div>
  );
}
