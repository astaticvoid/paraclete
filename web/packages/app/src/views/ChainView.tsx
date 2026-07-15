// SPDX-License-Identifier: GPL-3.0-or-later
// ChainView — convenience projection of the track's signal graph (W2 Commit 6).
// Shows nodes in signal-flow order; tap a node to navigate to its page group.
// Send sliders appear per routing destination.

import type { ViewMetaChain, ViewMetaChainRoute } from "@paraclete/core";
import type { Connection, StateStore } from "@paraclete/core";

export interface ChainViewProps {
  connection: Connection | null;
  stateStore: StateStore;
  chain: ViewMetaChain;
  /** Available page IDs from view_meta — used for navigation on tap. */
  pageIds: string[];
  onNavigate: (pageId: string) => void;
}

export function ChainView({ chain, pageIds, onNavigate }: ChainViewProps) {
  const labelMap = new Map(chain.node_labels);

  return (
    <div class="chain-view">
      <div class="chain-header">Signal Chain</div>

      <div class="chain-nodes">
        {chain.nodes.map((nid, i) => {
          const label = labelMap.get(nid) ?? `Node ${nid}`;
          const isLast = i === chain.nodes.length - 1;
          return (
            <div key={nid} class="chain-node-row">
              <button
                class="chain-node-btn"
                onClick={() => {
                  if (pageIds.length > 0) onNavigate(pageIds[0]);
                }}
                title={`Navigate to page`}
              >
                <span class="chain-node-idx">{i + 1}</span>
                <span class="chain-node-label">{label}</span>
              </button>
              {!isLast && <div class="chain-edge">→</div>}
            </div>
          );
        })}
      </div>

      {chain.routing && chain.routing.length > 0 && (
        <div class="chain-routing">
          <div class="chain-section-label">Routing</div>
          {chain.routing.map((r, ri) => (
            <div key={ri} class="chain-route">
              <span class="chain-route-source">
                {labelMap.get(r.source) ?? `Node ${r.source}`}
              </span>
              <span class="chain-route-arrow">→</span>
              <span class="chain-route-dest">{r.dest}</span>
              <span class="chain-route-param">{r.param_id}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
