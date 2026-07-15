// SPDX-License-Identifier: GPL-3.0-or-later
// Protocol v0 wire types — mirrors `crates/paraclete-antiphon/src/protocol.rs`
// (design/phases/w0-interfaces.md, w1-interfaces.md). Keep in lockstep with
// the Rust source of truth: field names and the "t" tag values are the wire
// contract. Variants marked [W1] are new in this commit.

export const PROTOCOL_VERSION = 0;

// ── Client → server ─────────────────────────────────────────────────────────

/** Must be the first frame of a session. */
export interface HelloMsg {
  t: "hello";
  token: string;
  client: string;
}

/** Pad ids: 0–63 grid (row*8+col), 64–71 scene, 72–79 control row. */
export interface PadDownMsg {
  t: "pad_down";
  id: number;
  vel: number;
}

export interface PadUpMsg {
  t: "pad_up";
  id: number;
}

/** `ts` is client-clock milliseconds, echoed verbatim in `pong`. */
export interface PingMsg {
  t: "ping";
  ts: number;
}

/** [W1+] continuous per-pad pressure (16-bit). Reserved; not sent by this
 * client yet (poly aftertouch is a later milestone). */
export interface PadPresMsg {
  t: "pad_pres";
  id: number;
  v: number;
}

/** [W1] encoder ids 90–97; delta = accumulated detents this frame. */
export interface EncMsg {
  t: "enc";
  id: number;
  delta: number;
}

/** [W1] */
export interface EncPushMsg {
  t: "enc_push";
  id: number;
  pressed: boolean;
}

/** [W1] semantic plane. */
export interface SetParamMsg {
  t: "set_param";
  node: number;
  param: string;
  v: number;
}

/** [W1] */
export interface BumpParamMsg {
  t: "bump_param";
  node: number;
  param: string;
  delta: number;
}

/** [W1] declared node commands; pass-through. Universal commands (cmd < 16)
 * are rejected server-side — use the typed messages above for those. */
export interface NodeCmdMsg {
  t: "node_cmd";
  node: number;
  cmd: number;
  a0: number;
  a1: number;
}

export type ClientMsg =
  | HelloMsg
  | PadDownMsg
  | PadUpMsg
  | PingMsg
  | PadPresMsg
  | EncMsg
  | EncPushMsg
  | SetParamMsg
  | BumpParamMsg
  | NodeCmdMsg
  | GetViewMetaMsg;

// ── Server → client ─────────────────────────────────────────────────────────

export interface ParamSummary {
  id: number;
  name: string;
  min: number;
  max: number;
  default: number;
}

export interface NodeSummary {
  id: number;
  type_tag: string;
  name: string;
  params: ParamSummary[];
  /** [W2] whether this node has a view (non-None view on the cap-doc). */
  has_view?: boolean;
}

export interface TransportSummary {
  playing: boolean;
  bpm: number;
}

export interface LedMsgEntry {
  id: number;
  rgb: [number, number, number];
}

/** [W1] one coalesced state-mirror change. */
export interface StateUpdate {
  path: string;
  v: number;
}

/** [W1] one resolved encoder→node/param mapping slot. `enc` is the encoder
 * SLOT INDEX 0-7 (trailing int of the profile's `encoder_{i}` context key),
 * not a surface control id — each client maps its own encoder controls
 * (this client: canvas cells / ids 90-97) onto slot indexes. */
export interface ContextSlot {
  enc: number;
  node: number;
  param: string;
}

export interface WelcomeMsg {
  t: "welcome";
  protocol: number;
  device_id: number;
  nodes: NodeSummary[];
  transport: TransportSummary;
}

/** Batched LED updates; a full-surface batch follows `welcome`. */
export interface LedMsg {
  t: "led";
  updates: LedMsgEntry[];
}

export interface PongMsg {
  t: "pong";
  ts: number;
}

/** Sent before the server closes the connection. */
export interface ByeMsg {
  t: "bye";
  reason: string;
}

/** [W1] coalesced state mirror, <= ~30 Hz. */
export interface StateMsg {
  t: "state";
  updates: StateUpdate[];
}

/** [W1] full 8-slot snapshot each time any slot changes. */
export interface ContextMsg {
  t: "context";
  slots: ContextSlot[];
}

/** [W1] same shape as `welcome.nodes`, sent after a topology patch. */
export interface TopologyMsg {
  t: "topology";
  nodes: NodeSummary[];
}

export type ServerMsg =
  | WelcomeMsg
  | LedMsg
  | PongMsg
  | ByeMsg
  | StateMsg
  | ContextMsg
  | TopologyMsg
  | ViewMetaMsg;

// ── W2: view_meta types ────────────────────────────────────────────────────

/** [W2] request composite page layout for a track. */
export interface GetViewMetaMsg {
  t: "get_view_meta";
  track_id: number;
  nonce?: string;
}

export interface ViewMetaParam {
  id: string;
  node_id: number;
  label: string;
  affordance: string;
  env_group?: number;
  slot: number;
  stepped?: boolean;
  options?: string[];
  routing?: { dest: string };
}

export interface ViewMetaEnvelope {
  id: number;
  type: string;
  label: string;
  param_ids: string[];
}

export interface ViewMetaMacro {
  name: string;
  targets: string[];
  page?: string;
}

export interface ViewMetaPage {
  id: string;
  label: string;
  params: ViewMetaParam[];
  envelopes?: ViewMetaEnvelope[];
  macros?: ViewMetaMacro[];
}

export interface ViewMetaChainRoute {
  source: number;
  dest: string;
  param_id: string;
  value: number;
}

export interface ViewMetaChain {
  nodes: number[];
  node_labels: [number, string][];
  routing?: ViewMetaChainRoute[];
}

/** [W2] composite page layout for a track. */
export interface ViewMetaMsg {
  t: "view_meta";
  track_id: number;
  nonce?: string;
  engine_node_id: number;
  engine_name: string;
  display_name: string;
  pages: ViewMetaPage[];
  chain: ViewMetaChain;
}
