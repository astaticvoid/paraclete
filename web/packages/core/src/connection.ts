// SPDX-License-Identifier: GPL-3.0-or-later
// WebSocket connection wrapper: token handshake, typed dispatch, ping/RTT,
// reconnect with backoff. Ported from the W0 vanilla client
// (web/theoria/theoria.js, deleted in this commit) — same wire behaviour,
// now transport-agnostic (no DOM access beyond the global WebSocket ctor).

import type { ClientMsg, ServerMsg } from "./protocol";
import { PROTOCOL_VERSION } from "./protocol";

const STALE_MS = 500;
const PING_INTERVAL_MS = 2000;
const BACKOFF_MS = [1000, 2000, 4000, 5000];

export type ConnectionStatus = "connecting" | "open" | "stale" | "error";

export interface ConnectionOptions {
  url: string;
  token: string;
  client: string;
  onMessage: (msg: ServerMsg) => void;
  onStatus?: (status: ConnectionStatus, detail?: string) => void;
}

/** Median-of-last-10 RTT + touch->LED latency, same instrumentation as W0. */
export class Connection {
  private ws: WebSocket | null = null;
  private reconnectAttempt = 0;
  private staleTimer: ReturnType<typeof setTimeout> | null = null;
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private hardError = false;
  private connected = false;
  private lastPongAt = 0;
  private rttSamples: number[] = [];

  constructor(private opts: ConnectionOptions) {}

  connect(): void {
    if (this.hardError) return;
    this.setStatus("connecting");
    const ws = new WebSocket(this.opts.url);
    this.ws = ws;
    if (this.staleTimer) clearTimeout(this.staleTimer);
    this.staleTimer = setTimeout(() => {
      if (!this.connected) this.setStatus("stale");
    }, STALE_MS);

    ws.addEventListener("open", () => {
      this.lastPongAt = now();
      this.rawSend({ t: "hello", token: this.opts.token, client: this.opts.client });
    });

    ws.addEventListener("message", (e: MessageEvent) => {
      let msg: ServerMsg;
      try {
        msg = JSON.parse(e.data as string);
      } catch {
        return;
      }
      this.handleMessage(msg);
    });

    ws.addEventListener("close", () => {
      this.connected = false;
      this.stopPing();
      this.setStatus("stale");
      if (this.hardError) return;
      const backoff = BACKOFF_MS[Math.min(this.reconnectAttempt, BACKOFF_MS.length - 1)];
      this.reconnectAttempt += 1;
      setTimeout(() => this.connect(), backoff);
    });

    ws.addEventListener("error", () => ws.close());
  }

  /** Hard-close: no reconnect (bad token / protocol mismatch). */
  closeHard(reason: string): void {
    this.hardError = true;
    this.setStatus("error", reason);
    this.ws?.close();
  }

  send(msg: ClientMsg): void {
    this.rawSend(msg);
  }

  medianRttMs(): number | null {
    return median(this.rttSamples);
  }

  private rawSend(msg: ClientMsg): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  private handleMessage(msg: ServerMsg): void {
    switch (msg.t) {
      case "welcome":
        if (msg.protocol !== PROTOCOL_VERSION) {
          this.closeHard(
            `protocol mismatch: server speaks v${msg.protocol}, this client v${PROTOCOL_VERSION}`,
          );
          return;
        }
        this.connected = true;
        this.reconnectAttempt = 0;
        if (this.staleTimer) clearTimeout(this.staleTimer);
        this.startPing();
        this.setStatus("open");
        break;
      case "pong": {
        this.lastPongAt = now();
        const rtt = now() - msg.ts;
        this.rttSamples.push(rtt);
        if (this.rttSamples.length > 10) this.rttSamples.shift();
        break;
      }
      case "bye":
        if (msg.reason === "bad token") {
          this.closeHard("bad or missing token");
        }
        // "full": keep the backoff loop running — a slot may free up.
        break;
      default:
        break;
    }
    this.opts.onMessage(msg);
  }

  private startPing(): void {
    this.stopPing();
    this.pingTimer = setInterval(() => {
      if (this.connected && now() - this.lastPongAt > 2.5 * PING_INTERVAL_MS) {
        this.ws?.close();
        return;
      }
      this.rawSend({ t: "ping", ts: now() });
    }, PING_INTERVAL_MS);
  }

  private stopPing(): void {
    if (this.pingTimer) clearInterval(this.pingTimer);
    this.pingTimer = null;
  }

  private setStatus(status: ConnectionStatus, detail?: string): void {
    this.opts.onStatus?.(status, detail);
  }
}

function now(): number {
  return typeof performance !== "undefined" ? performance.now() : Date.now();
}

function median(xs: number[]): number | null {
  if (xs.length === 0) return null;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.floor(s.length / 2)];
}
