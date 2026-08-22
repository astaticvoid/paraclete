#!/usr/bin/env bash
# SPIKE-003 (#94) prototype: workspace-internal layer-boundary check.
#
# clippy's disallowed-dependencies lint only covers external crate names, not
# path/workspace deps, so it can't see a violation like "paraclete-hal depends
# on paraclete-runtime" (BUG-003, since fixed). This script fills that gap by
# reading `cargo metadata` and checking each L0-L4 crate's *normal*
# dependencies (dev-dependencies are exempt — integration tests legitimately
# depend on concrete node implementations to exercise an abstract layer)
# against AGENTS.md's five-layer model.
#
# Rule encoded here (verified against the graph as of 2026-08-22): L0 HAL,
# L1 Runtime, L3 Nodes, and L4 Scripting may each depend on L2 Node API and
# nothing else paraclete-internal. L2 Node API itself may depend on no other
# paraclete-internal crate (it's the boundary). "Platform crates" (antiphon,
# clap*, tui, theotokos, view-assembly, graph-nodes, machine-*) and
# paraclete-app sit outside the strict layer table per AGENTS.md and are not
# restricted here.
#
# Advisory-only for now (exit 0 even on violation) — see #94 for the CI-gate
# decision. Run: tools/check-layers.sh

set -euo pipefail

RESTRICTED='paraclete-hal paraclete-runtime paraclete-nodes paraclete-scripting'
ALLOWED_TARGET='paraclete-node-api'

cd "$(dirname "$0")/.."

violations=0

while IFS=$'\t' read -r pkg dep; do
  for r in $RESTRICTED; do
    if [ "$pkg" = "$r" ] && [ "$dep" != "$ALLOWED_TARGET" ]; then
      echo "VIOLATION: $pkg (restricted layer) depends on $dep (must be $ALLOWED_TARGET only)"
      violations=$((violations + 1))
    fi
  done
  if [ "$pkg" = "$ALLOWED_TARGET" ]; then
    echo "VIOLATION: $ALLOWED_TARGET (the boundary crate) depends on $dep"
    violations=$((violations + 1))
  fi
done < <(
  cargo metadata --no-deps --format-version 1 2>/dev/null | python3 -c '
import json, sys
data = json.load(sys.stdin)
for pkg in data["packages"]:
    name = pkg["name"]
    if not name.startswith("paraclete"):
        continue
    for d in pkg["dependencies"]:
        dep_name = d["name"]
        if dep_name.startswith("paraclete") and d["kind"] is None:
            print(name + "\t" + dep_name)
'
)

if [ "$violations" -eq 0 ]; then
  echo "check-layers: no layer-boundary violations found."
else
  echo "check-layers: $violations violation(s) found."
fi
