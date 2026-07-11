// SPDX-License-Identifier: GPL-3.0-or-later
// Code-entry gate — shown when the server rejects the handshake token (or
// none is stored). The terminal prints a 6-digit session code; the human
// types it once here and it persists in localStorage, so the bare URL
// (`http://host:7274/`) is all a tablet ever needs (2026-07-10 ergonomics —
// the old 32-hex `?t=` URL was untypeable on glass and the bare URL just
// showed STALE forever).

import { useState } from "preact/hooks";

export interface TokenGateProps {
  onSubmit: (code: string) => void;
}

export function TokenGate({ onSubmit }: TokenGateProps) {
  const [code, setCode] = useState("");
  const ready = code.trim().length >= 4;

  function submit() {
    if (ready) onSubmit(code.trim());
  }

  return (
    <div class="token-gate">
      <div class="token-gate-card">
        <div class="token-gate-title">Paraclete</div>
        <div class="token-gate-hint">
          Enter the session code shown in the terminal
          <br />
          (the 6 digits after <code>code</code> in the Theoria line)
        </div>
        <input
          class="token-gate-input"
          type="text"
          inputMode="numeric"
          autocomplete="off"
          maxLength={32}
          placeholder="······"
          value={code}
          onInput={(e) => setCode((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
        />
        <button class="token-gate-join" disabled={!ready} onClick={submit}>
          JOIN
        </button>
      </div>
    </div>
  );
}
