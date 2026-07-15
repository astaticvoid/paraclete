// SPDX-License-Identifier: GPL-3.0-or-later
// ParamPage — renders one page of the contextual window (W2 Commit 4).
// 2-column grid of param cells; each cell shows name + value + value bar.
// Touch-drag on a cell sends bump_param (same semantic wire as encoder row).
// Affordance stubs render where the spec requires; full visualisation
// ships with the vertical slice (Commit 5) when real data is live.

import { useEffect, useRef, useState } from "preact/hooks";
import {
  COARSE_PX_PER_DETENT,
  DragToDetents,
  FINE_PX_PER_DETENT,
  type Connection,
  type NodeSummary,
  type StateStore,
  type ViewMetaPage,
} from "@paraclete/core";

const DETENTS_PER_RANGE = 256;
const COLS = 2;

export interface ParamPageProps {
  connection: Connection | null;
  stateStore: StateStore;
  page: ViewMetaPage;
  nodes: NodeSummary[];
}

function paramRange(nodes: NodeSummary[], nodeId: number, paramId: string) {
  const node = nodes.find((n) => n.id === nodeId);
  if (!node) return { min: 0, max: 1 };
  const p = node.params.find((pr) => pr.name === paramId);
  return p ? { min: p.min, max: p.max } : { min: 0, max: 1 };
}

export function ParamPage({ connection, stateStore, page, nodes }: ParamPageProps) {
  const [, setTick] = useState(0);
  const dragRef = useRef<{
    param: { nodeId: number; paramId: string; range: { min: number; max: number } };
    drag: DragToDetents;
    lastY: number;
  } | null>(null);

  useEffect(() => {
    return stateStore.subscribe(() => setTick((t) => t + 1));
  }, [stateStore]);

  const sortedParams = [...page.params].sort((a, b) => a.slot - b.slot);

  const nonEnvParams = sortedParams.filter(
    (p) => p.affordance !== "EnvelopeCurve",
  );

  const envParams = sortedParams.filter(
    (p) => p.affordance === "EnvelopeCurve",
  );

  if (nonEnvParams.length === 0 && envParams.length === 0) {
    return (
      <div class="param-page">
        <div class="page-placeholder">
          <div class="page-hint">no parameters on this page</div>
        </div>
      </div>
    );
  }

  return (
    <div class="param-page">
      {page.envelopes && page.envelopes.length > 0 && envParams.length > 0 && (
        <div class="param-envelope-view">
          <span class="param-envelope-label">
            {page.envelopes[0]?.label ?? "Envelope"}
          </span>
          <svg class="param-envelope-svg" viewBox="0 0 200 80" preserveAspectRatio="none">
            <EnvelopeCurve
              pageEnvelopes={page.envelopes}
              envParams={envParams}
              stateStore={stateStore}
              nodes={nodes}
            />
          </svg>
        </div>
      )}

      <div class="param-grid">
        {nonEnvParams.map((p) => {
          const path = `/node/${p.node_id}/param/${p.id}`;
          const value = stateStore.get(path);
          const range = paramRange(nodes, p.node_id, p.id);
          const frac =
            value !== undefined && range.max > range.min
              ? Math.max(0, Math.min(1, (value - range.min) / (range.max - range.min)))
              : 0;

          return (
            <div
              key={`${p.node_id}:${p.id}`}
              class="param-cell"
              onPointerDown={(e) => {
                const fine = e.shiftKey;
                const drag = new DragToDetents(fine ? FINE_PX_PER_DETENT : COARSE_PX_PER_DETENT);
                dragRef.current = {
                  param: { nodeId: p.node_id, paramId: p.id, range },
                  drag,
                  lastY: e.clientY,
                };
                try {
                  e.currentTarget.setPointerCapture(e.pointerId);
                } catch {
                  /* no-op */
                }
              }}
              onPointerMove={(e) => {
                const d = dragRef.current;
                if (!d) return;
                const dy = e.clientY - d.lastY;
                d.lastY = e.clientY;
                const detents = d.drag.feed(dy);
                if (detents !== 0 && connection) {
                  const delta =
                    detents * ((d.param.range.max - d.param.range.min) / DETENTS_PER_RANGE);
                  connection.send({
                    t: "bump_param",
                    node: d.param.nodeId,
                    param: d.param.paramId,
                    delta,
                  });
                }
              }}
              onPointerUp={() => {
                dragRef.current = null;
              }}
              onPointerCancel={() => {
                dragRef.current = null;
              }}
            >
              <span class="param-name">{p.label}</span>
              <span class="param-value">
                {value !== undefined
                  ? p.affordance === "FilterShape"
                    ? `${value.toFixed(0)} Hz`
                    : formatValue(value)
                  : "—"}
              </span>
              <div class="param-bar-track">
                <div
                  class={`param-bar-fill ${p.affordance === "FilterShape" ? "filter-shape" : ""}`}
                  style={{ width: `${frac * 100}%` }}
                />
              </div>
              {/* Affordance stubs — graphical renderers ship in Commit 5 */}
              {p.affordance === "FilterShape" && (
                <div class="param-affordance-stub filter">filter curve — Commit 5</div>
              )}
              {p.affordance === "LfoShape" && (
                <div class="param-affordance-stub lfo">lfo shape — Commit 5</div>
              )}
              {p.affordance === "Waveform" && (
                <div class="param-affordance-stub waveform">waveform — Commit 5</div>
              )}
              {p.affordance === "DiagramHighlight" && (
                <div class="param-affordance-stub diagram">diagram — Commit 5</div>
              )}
              {p.stepped && p.options && (
                <div class="param-stepped-hint">
                  {p.options.join(" | ")}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {page.macros && page.macros.length > 0 && (
        <div class="param-macros">
          {page.macros.map((m, mi) => (
            <div key={mi} class="param-macro">
              <span class="param-macro-name">{m.name}</span>
              <span class="param-macro-targets">{m.targets.join(" · ")}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function formatValue(v: number): string {
  const a = Math.abs(v);
  if (a >= 100) return v.toFixed(0);
  if (a >= 10) return v.toFixed(1);
  if (a >= 1) return v.toFixed(2);
  return v.toFixed(3);
}

function EnvelopeCurve({
  pageEnvelopes,
  envParams,
  stateStore,
  nodes,
}: {
  pageEnvelopes: { id: number; type: string; label: string; param_ids: string[] }[];
  envParams: { id: string; node_id: number; label: string; env_group?: number }[];
  stateStore: StateStore;
  nodes: NodeSummary[];
}) {
  return (
    <>
      {pageEnvelopes.map((env) => {
        // Find params matching this envelope group's param_ids
        const values = env.param_ids.map((pid) => {
          const paramEntry = envParams.find((p) => p.id === pid);
          if (!paramEntry) return 0.5;
          const path = `/node/${paramEntry.node_id}/param/${pid}`;
          return stateStore.get(path) ?? 0.5;
        });

        // ADSR/AD envelope shape
        const envType = env.type;
        if (envType === "AD" || envType === "AHD") {
          const attack = values[0] ?? 0.5;
          const decay = values[1] ?? (values[0] ?? 0.5);
          const attackEnd = Math.min(attack * 400, 120);
          const decayEnd = Math.min(attackEnd + decay * 400, 200);
          const points = `0,80 ${attackEnd},10 ${decayEnd},80`;
          return (
            <g key={env.id}>
              <title>{env.label}</title>
              <polyline
                points={points}
                fill="none"
                stroke="#4a6"
                stroke-width="2"
                vector-effect="non-scaling-stroke"
              />
            </g>
          );
        }

        // Default: ADSR
        const [a, d, s] = values;
        const r = values[3] ?? (values[1] ?? 0.5);
        const attackEnd = Math.min(a * 300, 80);
        const decayEnd = Math.min(attackEnd + d * 300, 140);
        const sustainY = 80 - s * 70;
        const releaseStart = 140;
        const releaseEnd = Math.min(releaseStart + r * 300, 200);
        return (
          <g key={env.id}>
            <title>{env.label}</title>
            <polyline
              points={`0,80 ${attackEnd},10 ${decayEnd},${sustainY} ${releaseStart},${sustainY} ${releaseEnd},80`}
              fill="none"
              stroke="#4a6"
              stroke-width="2"
              vector-effect="non-scaling-stroke"
            />
          </g>
        );
      })}
    </>
  );
}
