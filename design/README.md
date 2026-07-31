# `design/` — what lives here, and what moved

**2026-07-30: the mutable planning trackers moved to GitHub Issues.**

`design/` now holds only append-only design *authority* and *record*. Anything
that describes "what is true right now" — open bugs, open questions, spec gaps,
spikes, provisional implementations, session carryover — is an issue.

## Where things are

| You want… | Look in |
|---|---|
| Open bugs, open questions, spikes, spec gaps | **GitHub Issues** |
| What to work on next | the open **milestone** — `gh issue list --milestone TK2.2` |
| Why a decision was made | `design/adr/` (index: `adr/INDEX.md`) |
| What a phase specified and what shipped | `design/phases/` |
| Paired-session findings | `design/sessions/` |
| Post-phase code reviews and audits | `design/review/` |
| The phase sequence and design gates | `design/roadmap.md` |
| Architecture reference | `design/architecture-core.md` |

## Resolving a `BUG-###` / `INFRA-###` from a code comment

Source comments reference bug IDs by number. They are preserved verbatim in
issue titles, so:

```bash
gh issue list --search "BUG-042 in:title" --state all
gh issue view <n>
```

`ADR-###` references still resolve **in-repo** via `design/adr/INDEX.md` — ADRs
did not move, and are cited from published crate metadata.

## What moved, and where its history is

| Was | Now |
|---|---|
| `design/bugs.md` | 58 issues (`BUG-###` / `INFRA-###` in the title) + 5 `history` issues |
| `design/roadmap.md` open-question / spike / spec-gap / provisional / agent-gap tables | issues, labelled `open-question` / `spike` / `spec-gap` / `provisional` / `agent-infra` |
| `design/roadmap.md` 471-line header blockquote | deleted — it was a hand-maintained changelog of a git-versioned file |
| `design/handoff.md` | guardrails + task routing → `AGENTS.md`; the "▶ START HERE" pointer → the open milestone |
| `design/todo-scratch.md` | issues, labelled `carryover` |
| Theotokos `OQ-T` register | issues, labelled `open-question` + `theotokos`. Design rationale stays in `design/theotokos/design.md` |

Full prose of every migrated entry is in its issue; the deleted files remain in
git history (`git log --follow -- design/bugs.md`).

**Historical cross-references.** Documents written before this date — ADRs,
phase specs, session notes — link to `design/bugs.md` and `design/roadmap.md`
sections that no longer exist. Those documents are append-only and were **not**
rewritten. Resolve such a reference by searching issues for the ID, or by
reading the file at the commit that cited it.

## Rules

- **ADRs**: the decision/context/alternatives body is append-only. The
  `Status:` line and an appended implementation note *are* updated on
  implementation.
- **Phase specs and reports**: append-only.
- **`roadmap.md`**: plan, not state. Never reintroduce a status block or a
  `Previous:` revision stack.
- **Defects found during design or review work** are filed as issues against
  the code, never worked around in prose.
