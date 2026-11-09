// SPDX-License-Identifier: GPL-3.0-or-later
// PatchView — Visual modular node graph and patch cable editor (W5).
// Renders the full topology (nodes + input/output ports + edges), provides
// interactive cable routing (tap output -> tap input), node spawning, and deletion.

import { useState, useMemo } from "preact/hooks";
import type { Connection, EdgeSummary, NodeSummary, PortSummary, TopologyPatchOp } from "@paraclete/core";

export interface PatchViewProps {
  connection: Connection | null;
  nodes: NodeSummary[];
  edges: EdgeSummary[];
  onRefreshTopology: () => void;
}

const AVAILABLE_NODE_TYPES = [
  { tag: "vca", label: "VCA (Lin/Exp Attenuverter)" },
  { tag: "filter", label: "State Variable Filter (SVF)" },
  { tag: "ladder_filter", label: "Ladder Filter (4-pole Moog style)" },
  { tag: "chorus", label: "Stereo Chorus / Flanger" },
  { tag: "compressor", label: "Stereo Compressor (Sidechain + GR)" },
  { tag: "delay", label: "Delay (Stereo Ping-Pong / Dub)" },
  { tag: "reverb", label: "Algorithmic Reverb" },
  { tag: "distortion", label: "Wavefolder / Distortion" },
  { tag: "oscillator", label: "Oscillator (Multi-wave)" },
  { tag: "envelope", label: "Envelope Generator (ADSR)" },
  { tag: "lfo", label: "LFO (Multi-wave modulation)" },
  { tag: "slew", label: "Slew Limiter (Glide / Portamento)" },
  { tag: "sample_hold", label: "Sample & Hold / Noise" },
  { tag: "quantizer", label: "Scale Quantizer" },
  { tag: "clock_divider", label: "Clock Divider / Multiplier" },
  { tag: "cv_mixer", label: "CV Mixer (4-channel bipolar)" },
  { tag: "mix", label: "Mixer (8-channel stereo audio)" },
  { tag: "split", label: "Splitter (1-in 2-out buffer)" },
  { tag: "analog_engine:kick", label: "Analog Drum: Kick" },
  { tag: "analog_engine:snare", label: "Analog Drum: Snare" },
  { tag: "analog_engine:hihat", label: "Analog Drum: HiHat" },
  { tag: "analog_engine:clap", label: "Analog Drum: Clap" },
  { tag: "analog_engine:tom", label: "Analog Drum: Tom" },
  { tag: "fm_engine:bass", label: "FM Engine: Bass" },
  { tag: "fm_voice", label: "FM Voice (4-op melodic poly)" },
];

function portColor(portType: string): string {
  switch (portType.toLowerCase()) {
    case "audio":
      return "#38bdf8"; // sky blue
    case "cv":
      return "#facc15"; // yellow / amber
    case "clock":
      return "#fb923c"; // orange
    case "logic":
      return "#c084fc"; // purple
    case "midi":
    case "midi2":
      return "#34d399"; // emerald green
    default:
      return "#94a3b8"; // slate
  }
}

interface PendingCable {
  srcNode: number;
  srcPort: number;
  portName: string;
  portType: string;
}

export function PatchView({
  connection,
  nodes,
  edges,
  onRefreshTopology,
}: PatchViewProps) {
  const [selectedType, setSelectedType] = useState<string>("vca");
  const [pendingCable, setPendingCable] = useState<PendingCable | null>(null);
  const [searchTerm, setSearchTerm] = useState<string>("");
  const [exportOpen, setExportOpen] = useState<boolean>(false);

  const nodeMap = useMemo(() => {
    const map = new Map<number, NodeSummary>();
    for (const n of nodes) {
      map.set(n.id, n);
    }
    return map;
  }, [nodes]);

  function sendPatch(changes: TopologyPatchOp[]) {
    if (!connection) return;
    connection.send({ t: "patch", changes });
  }

  function handleAddNode() {
    sendPatch([{ op: "add_node", type_tag: selectedType }]);
  }

  function handleRemoveNode(id: number) {
    if (confirm(`Remove node #${id}? Any connected cables will be detached.`)) {
      sendPatch([{ op: "remove_node", id }]);
    }
  }

  function handlePortClick(node: NodeSummary, port: PortSummary) {
    const isOutput = port.direction.toLowerCase() === "output";
    if (isOutput) {
      if (
        pendingCable &&
        pendingCable.srcNode === node.id &&
        pendingCable.srcPort === port.id
      ) {
        setPendingCable(null);
      } else {
        setPendingCable({
          srcNode: node.id,
          srcPort: port.id,
          portName: port.name,
          portType: port.port_type,
        });
      }
    } else {
      // Input clicked
      if (pendingCable) {
        if (pendingCable.srcNode === node.id) {
          alert("Cannot connect a node to itself directly.");
          return;
        }
        sendPatch([
          {
            op: "add_edge",
            src: pendingCable.srcNode,
            src_port: pendingCable.srcPort,
            dst: node.id,
            dst_port: port.id,
          },
        ]);
        setPendingCable(null);
      }
    }
  }

  function handleRemoveEdge(e: EdgeSummary) {
    sendPatch([
      {
        op: "remove_edge",
        src: e.src_node,
        src_port: e.src_port,
        dst: e.dst_node,
        dst_port: e.dst_port,
      },
    ]);
  }

  const filteredNodes = useMemo(() => {
    if (!searchTerm.trim()) return nodes;
    const term = searchTerm.toLowerCase();
    return nodes.filter(
      (n) =>
        (n.name ?? "").toLowerCase().includes(term) ||
        (n.type_tag && n.type_tag.toLowerCase().includes(term)) ||
        n.id.toString() === term,
    );
  }, [nodes, searchTerm]);

  const yamlSummary = useMemo(() => {
    const lines = ["# Exported Paraclete Signal Graph Topology", "nodes:"];
    for (const n of nodes) {
      lines.push(`  - id: ${n.id}`);
      lines.push(`    type: ${n.type_tag ?? (n.name ?? "").toLowerCase()}`);
    }
    lines.push("edges:");
    for (const e of edges) {
      lines.push(`  - from: [${e.src_node}, ${e.src_port}]`);
      lines.push(`    to:   [${e.dst_node}, ${e.dst_port}]`);
    }
    return lines.join("\n");
  }, [nodes, edges]);

  return (
    <div class="patch-view">
      {/* Patch Toolbar */}
      <div class="patch-toolbar">
        <div class="patch-toolbar-title">
          <span class="patch-title-tag">PATCH</span>
          <span class="patch-title-desc">Modular Graph Editor</span>
        </div>

        <div class="patch-add-group">
          <select
            class="patch-select"
            value={selectedType}
            onChange={(e) => setSelectedType((e.target as HTMLSelectElement).value)}
          >
            {AVAILABLE_NODE_TYPES.map((t) => (
              <option key={t.tag} value={t.tag}>
                {t.label}
              </option>
            ))}
          </select>
          <button class="patch-btn patch-btn-primary" onClick={handleAddNode}>
            + Add Node
          </button>
        </div>

        <div class="patch-search-group">
          <input
            type="text"
            class="patch-input"
            placeholder="Filter nodes..."
            value={searchTerm}
            onInput={(e) => setSearchTerm((e.target as HTMLInputElement).value)}
          />
        </div>

        <div class="patch-action-group">
          <button class="patch-btn" onClick={onRefreshTopology} title="Sync topology from server">
            ↻ Sync
          </button>
          <button
            class="patch-btn"
            onClick={() => setExportOpen(!exportOpen)}
            title="Export YAML representation"
          >
            {exportOpen ? "Hide YAML" : "Export"}
          </button>
        </div>
      </div>

      {/* Cable in flight banner */}
      {pendingCable && (
        <div class="patch-cable-banner">
          <span class="cable-dot" style={{ background: portColor(pendingCable.portType) }} />
          <span>
            Connecting Cable from <strong>#{pendingCable.srcNode}</strong>:
            <em>{pendingCable.portName}</em> ({pendingCable.portType}) → Click an input port on any
            destination node to complete the patch.
          </span>
          <button class="patch-btn patch-btn-sm" onClick={() => setPendingCable(null)}>
            Cancel [Esc]
          </button>
        </div>
      )}

      {/* YAML Export Tray */}
      {exportOpen && (
        <div class="patch-export-tray">
          <div class="patch-export-header">
            <span>Graph YAML Specification</span>
            <button
              class="patch-btn patch-btn-sm"
              onClick={() => {
                navigator.clipboard?.writeText(yamlSummary);
                alert("YAML copied to clipboard!");
              }}
            >
              Copy to Clipboard
            </button>
          </div>
          <pre class="patch-export-content">{yamlSummary}</pre>
        </div>
      )}

      {/* Main Graph Grid Canvas */}
      <div class="patch-canvas">
        <div class="patch-nodes-grid">
          {filteredNodes.map((node) => {
            const ports = node.ports ?? [];
            const inPorts = ports.filter((p) => p.direction.toLowerCase() === "input");
            const outPorts = ports.filter((p) => p.direction.toLowerCase() === "output");

            return (
              <div key={node.id} class="patch-node-card">
                <div class="patch-node-header">
                  <div class="patch-node-id">#{node.id}</div>
                  <div class="patch-node-name" title={node.type_tag ?? node.name}>
                    {node.name || node.type_tag}
                  </div>
                  <button
                    class="patch-node-close-btn"
                    onClick={() => handleRemoveNode(node.id)}
                    title={`Delete node #${node.id}`}
                  >
                    ✕
                  </button>
                </div>

                <div class="patch-node-body">
                  {/* Inputs Column */}
                  <div class="patch-ports-col patch-inputs-col">
                    <div class="patch-port-col-label">IN</div>
                    {inPorts.map((p) => {
                      const isConnected = edges.some(
                        (e) => e.dst_node === node.id && e.dst_port === p.id,
                      );
                      const isTargetCandidate =
                        pendingCable !== null && pendingCable.srcNode !== node.id;

                      return (
                        <button
                          key={p.id}
                          class={`patch-port-btn in-port ${isConnected ? "connected" : ""} ${
                            isTargetCandidate ? "target-pulse" : ""
                          }`}
                          onClick={() => handlePortClick(node, p)}
                          title={`Input #${p.id}: ${p.name} (${p.port_type})`}
                        >
                          <span
                            class="patch-port-jack"
                            style={{
                              borderColor: portColor(p.port_type),
                              background: isConnected ? portColor(p.port_type) : "transparent",
                            }}
                          />
                          <span class="patch-port-name">{p.name}</span>
                        </button>
                      );
                    })}
                    {inPorts.length === 0 && <span class="patch-ports-empty">—</span>}
                  </div>

                  {/* Outputs Column */}
                  <div class="patch-ports-col patch-outputs-col">
                    <div class="patch-port-col-label">OUT</div>
                    {outPorts.map((p) => {
                      const isConnected = edges.some(
                        (e) => e.src_node === node.id && e.src_port === p.id,
                      );
                      const isPending =
                        pendingCable?.srcNode === node.id && pendingCable?.srcPort === p.id;

                      return (
                        <button
                          key={p.id}
                          class={`patch-port-btn out-port ${isConnected ? "connected" : ""} ${
                            isPending ? "active-source" : ""
                          }`}
                          onClick={() => handlePortClick(node, p)}
                          title={`Output #${p.id}: ${p.name} (${p.port_type})`}
                        >
                          <span class="patch-port-name">{p.name}</span>
                          <span
                            class="patch-port-jack"
                            style={{
                              borderColor: portColor(p.port_type),
                              background:
                                isConnected || isPending
                                  ? portColor(p.port_type)
                                  : "transparent",
                            }}
                          />
                        </button>
                      );
                    })}
                    {outPorts.length === 0 && <span class="patch-ports-empty">—</span>}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* Active Cables Strip */}
      <div class="patch-cables-panel">
        <div class="patch-cables-header">
          <span>Active Patch Cables ({edges.length})</span>
        </div>
        <div class="patch-cables-list">
          {edges.map((e, idx) => {
            const srcNode = nodeMap.get(e.src_node);
            const dstNode = nodeMap.get(e.dst_node);
            const srcPort = srcNode?.ports?.find((p) => p.id === e.src_port);
            const dstPort = dstNode?.ports?.find((p) => p.id === e.dst_port);

            const srcLabel = srcNode ? `#${srcNode.id} (${srcNode.name || srcNode.type_tag})` : `#${e.src_node}`;
            const dstLabel = dstNode ? `#${dstNode.id} (${dstNode.name || dstNode.type_tag})` : `#${e.dst_node}`;
            const portType = srcPort?.port_type ?? dstPort?.port_type ?? "Signal";

            return (
              <div key={idx} class="patch-cable-pill">
                <span class="cable-dot" style={{ background: portColor(portType) }} />
                <span class="cable-text">
                  <strong>{srcLabel}</strong>:{srcPort?.name ?? e.src_port} →{" "}
                  <strong>{dstLabel}</strong>:{dstPort?.name ?? e.dst_port}
                </span>
                <button
                  class="cable-remove-btn"
                  onClick={() => handleRemoveEdge(e)}
                  title="Disconnect cable"
                >
                  ✕
                </button>
              </div>
            );
          })}
          {edges.length === 0 && (
            <div class="patch-cables-none">No active patch cables in the graph.</div>
          )}
        </div>
      </div>
    </div>
  );
}
