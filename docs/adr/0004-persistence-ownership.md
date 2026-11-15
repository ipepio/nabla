# 0004 — Persistence ownership split

- Status: Accepted
- Scope: Persistence / architecture
- Acceptance evidence: `D-007` and `D-008` (confirmed during the `nabla-foundation` implementation),
  derived from ODD `odd/tasks/nabla-foundation.md` `S2`, `S3` and `S5`.
- Supersedes / superseded by: none

## Context

Nabla has four kinds of persistent state with different owners: what the human configures, what the
application accumulates, the transcripts Pi writes, and (in the future) agent memory. Storing them in
one place is the convenient default and the known failure mode: a shared database becomes an
integration point nobody owns. The rule the maintainer set is "files are for what the human
configures; the database is for what the app accumulates".

## Decision

Split storage by owner, never by convenience:

| State | Owner | Location |
| --- | --- | --- |
| Human-editable preferences | The maintainer | `settings.json` in Tauri's `app_config_dir()`, camelCase, atomic write in `src-tauri/src/settings.rs` |
| Application state (workspaces, session index) | The app | `nabla.db` (SQLite) in `app_data_dir()`, `src-tauri/src/store.rs` |
| Session transcripts | Pi | Pi's own JSONL files; the index stores only `pi_session_path` plus metadata |
| Agent memory | The agents | A future store behind the extensions/providers seam (`D-008`), not inside `nabla.db` |

Two invariants follow: the database is a rebuildable projection — on conflict with Pi's files the
files win, and the index must be reconstructible by scanning them — and no conversation content is
ever copied into `nabla.db`.

## Alternatives

One SQLite database for everything was rejected: it couples the app schema to Pi's transcript format
and to a memory store that does not exist yet. Storing configuration in the database was rejected:
`settings.json` must stay hand-editable and survive a deleted database. Embedding Engram was rejected
in `S5`: Engram is the pattern reference ("each owner keeps its own storage"), not a dependency.

## Consequences

- Losing `nabla.db` loses index rows and workspace bookkeeping, not sessions; a rebuild scan is the
  recovery path, still to be implemented.
- Schema changes never require a Pi change, and a Pi upgrade never requires a migration.
- Cross-store queries are impossible by design: joins happen in Rust over two owners' data.
- The agent-memory capability needs its own storage decision later; this record only forbids reusing
  the app store.

## Conditions to revisit

- When the agent-memory capability starts, `D-008` needs its own ADR: storage engine, scope and the
  seam through which the agent reaches it.
- If `settings.json` becomes insufficient for hand-editable configuration (secrets, per-workspace
  overrides), that is a new decision; `T-004` (keep secrets out of the frontend) is still open.
