// SPDX-License-Identifier: GPL-3.0-or-later
// KitsView — the KIT tab (P11 C7): kit list with load/save/commit/reload,
// temp save/reload, a perform-mode toggle, and per-pattern kit bindings.
//
// Data sources (all over the existing mirror — no new plumbing):
//   `/context/kits`        → kit list   (Text, via a context path slot)
//   `/context/kit_binding` → bindings   (Text, via a context path slot)
//   `/context/perform`     → perform on (Float, via a context path slot)
//   `list_kits`            → refresh/prime; the reply re-reads
//                            `/context/kits` at query time and the mirror is
//                            authoritative whenever it next lands.
//
// Every action is one of the P11 C7 app-op verbs, sent as a typed ClientMsg.

import { useEffect, useState } from "preact/hooks";
import type { Connection, ContextStore, MessageBus } from "@paraclete/core";
import {
  BINDING_SLOT_COUNT,
  KIT_SLOT_COUNT,
  defaultKitName,
  parseKitBinding,
  parseKitLine,
} from "../kits";

const PATH_KITS = "/context/kits";
const PATH_BINDING = "/context/kit_binding";
const PATH_PERFORM = "/context/perform";

export interface KitsViewProps {
  connection: Connection | null;
  contextStore: ContextStore;
  bus: MessageBus;
}

export function KitsView({ connection, contextStore, bus }: KitsViewProps) {
  const [kitsLine, setKitsLine] = useState("");
  const [bindingLine, setBindingLine] = useState("");
  const [performOn, setPerformOn] = useState<boolean | undefined>(undefined);
  const [saveName, setSaveName] = useState("");

  useEffect(() => {
    const refreshContext = () => {
      // The context mirror is the primary source; a `kit_list` reply below
      // only primes the list until the next snapshot lands.
      const kits = contextStore.text(PATH_KITS);
      if (kits !== undefined) setKitsLine(kits);
      const binding = contextStore.text(PATH_BINDING);
      if (binding !== undefined) setBindingLine(binding);
      const perform = contextStore.num(PATH_PERFORM);
      setPerformOn(perform !== undefined ? perform >= 0.5 : undefined);
    };
    const unsubContext = contextStore.subscribe(refreshContext);
    const unsubBus = bus.subscribe((msg) => {
      if (msg.t === "kit_list") setKitsLine(msg.kits);
    });
    refreshContext();
    // Prime the list on open; the REFRESH button re-sends it.
    connection?.send({ t: "list_kits" });
    return () => {
      unsubContext();
      unsubBus();
    };
  }, [connection, contextStore, bus]);

  const kits = parseKitLine(kitsLine);
  const kitBySlot = new Map(kits.map((k) => [k.slot, k.name]));
  const binding = parseKitBinding(bindingLine);
  const defaultName = defaultKitName(kits);

  function loadKit(slot: number) {
    connection?.send({ t: "kit_load", kit_id: slot });
  }

  function saveKit() {
    const name = saveName.trim() || defaultName;
    connection?.send({ t: "kit_save", name });
    setSaveName("");
  }

  function bindSlot(slot: number, kitId: number | null) {
    connection?.send({ t: "bind_kit", slot, kit_id: kitId });
  }

  function togglePerform() {
    if (performOn === undefined) return;
    connection?.send({ t: "set_perform_mode", on: !performOn });
  }

  return (
    <div class="kits-view">
      <div class="kits-toolbar">
        <div class="kits-save">
          <input
            class="kits-input"
            type="text"
            value={saveName}
            placeholder={defaultName}
            aria-label="kit save name"
            onInput={(e) => setSaveName((e.target as HTMLInputElement).value)}
          />
          <button class="kits-btn" onClick={saveKit}>
            SAVE
          </button>
        </div>
        <button class="kits-btn" onClick={() => connection?.send({ t: "kit_commit" })}>
          COMMIT
        </button>
        <button class="kits-btn" onClick={() => connection?.send({ t: "kit_reload" })}>
          RELOAD
        </button>
        <button class="kits-btn" onClick={() => connection?.send({ t: "temp_save" })}>
          TEMP SAVE
        </button>
        <button class="kits-btn" onClick={() => connection?.send({ t: "temp_reload" })}>
          TEMP RELOAD
        </button>
        <label
          class={`perform-toggle ${performOn === undefined ? "dimmed" : ""}`}
          title="Perform mode: pattern switches skip bound-kit apply"
        >
          <input
            type="checkbox"
            checked={performOn === true}
            disabled={performOn === undefined}
            onChange={togglePerform}
          />
          <span>PERFORM</span>
          <span class={`perform-state ${performOn ? "on" : "off"}`}>
            {performOn === undefined ? "—" : performOn ? "ON" : "OFF"}
          </span>
        </label>
        <button
          class="kits-btn"
          onClick={() => connection?.send({ t: "list_kits" })}
          title="Re-query the kit list"
        >
          REFRESH
        </button>
      </div>

      <div class="kits-panels">
        <section class="kits-panel">
          <h2 class="kits-panel-title">Kits · {KIT_SLOT_COUNT}</h2>
          <div class="kits-list">
            {Array.from({ length: KIT_SLOT_COUNT }, (_, slot) => {
              const name = kitBySlot.get(slot);
              return (
                <div key={slot} class={`kit-row ${name === undefined ? "empty" : ""}`}>
                  <span class="kit-idx">{slot + 1}</span>
                  <span class="kit-name">{name ?? "(empty)"}</span>
                  {name !== undefined && (
                    <button class="kit-load" onClick={() => loadKit(slot)}>
                      LOAD
                    </button>
                  )}
                </div>
              );
            })}
          </div>
        </section>

        <section class="kits-panel">
          <h2 class="kits-panel-title">Pattern Bindings</h2>
          <div class="kits-bind-list">
            {Array.from({ length: BINDING_SLOT_COUNT }, (_, slot) => (
              <div key={slot} class="bind-row">
                <span class="bind-idx">{slot + 1}</span>
                <select
                  class="kits-select"
                  value={binding[slot] === null ? "" : String(binding[slot])}
                  aria-label={`pattern slot ${slot + 1} kit binding`}
                  onChange={(e) => {
                    const v = (e.target as HTMLSelectElement).value;
                    bindSlot(slot, v === "" ? null : Number(v));
                  }}
                >
                  <option value="">(unbound)</option>
                  {kits.map((k) => (
                    <option key={k.slot} value={String(k.slot)}>
                      {k.slot + 1} · {k.name}
                    </option>
                  ))}
                </select>
              </div>
            ))}
          </div>
        </section>
      </div>
    </div>
  );
}
