# Architecture decision records

ADRs record the durable choices behind Nabla: why the desktop app is shaped the way it is. They are
promoted from the local decision register (`.docs/02-decisiones.md`, gitignored per
[ADR 0002](0002-local-definition-vs-tracked-docs.md)) so that agents and future maintainers can read
the reasoning without the local product definition.

An ADR is not a feature specification and creating one authorizes no implementation. Current work is
tracked with ODD in `odd/tasks/` (see [docs/ai/workflow.md](../ai/workflow.md)); the specs there are
authoritative for behavior, an ADR is authoritative for direction.

## Index

| ADR | Status | Decision |
| --- | --- | --- |
| [0001 — Desktop app on Tauri v2, Vue and a Pi-based harness](0001-desktop-tauri-vue-pi.md) | Accepted | One desktop application: Rust host owns processes and storage, Vue renders, Pi is the agent engine |
| [0002 — Local definition stays untracked, `docs/` is the tracked layer](0002-local-definition-vs-tracked-docs.md) | Accepted | `.docs/` local and gitignored; `docs/` versioned and agent-facing |
| [0003 — Product evolution order](0003-product-evolution-order.md) | Accepted | General assistant, then agent coordination, then development environment |
| [0004 — Persistence ownership split](0004-persistence-ownership.md) | Accepted | Files for human config, SQLite for app state, pi owns transcripts, agent memory gets its own store |
| [0005 — SQLite store implementation](0005-sqlite-store.md) | Accepted | `rusqlite` bundled, `PRAGMA user_version` migrations, WAL, foreign keys with cascade |
| [0006 — Pi runtime integration](0006-pi-runtime-integration.md) | Provisional | RPC-first: host-owned, supervised, one headless pi process per session, dies with the app |
| [0007 — Webview security baseline](0007-webview-security-baseline.md) | Accepted | CSP enabled from day one; `style-src 'unsafe-inline'` only for Vuetify's dynamic styles |
| [0008 — Application identifier](0008-application-identifier.md) | Accepted (value provisional) | `com.ipepio.nabla`, product name `Nabla`; determines the app-data directory |
| [0009 — Configurable theme tokens and a single brand module](0009-configurable-theme-tokens.md) | Accepted | Colors from `.env` (`VITE_COLOR_*`) through `src/lib/theme.ts`; brand images only through `src/lib/brand.ts` |

## Conventions

1. Copy [the ADR template](../templates/adr.md) to the next unused `NNNN-short-decision.md`. Use a
   short, decision-shaped title (`desktop-tauri-vue-pi`, not `decisions-about-the-stack`).
2. Keep one decision per record. A record that needs two unrelated `Decision` paragraphs is two
   records. Related source decisions may be merged when they describe one product shape (0001 does).
3. Every record cites its provenance in `Acceptance evidence`: the local decision IDs (`D-###`) or the
   ODD specs (`S#` of `odd/tasks/<feature>.md`) it derives from. No fact without a source.
4. Status vocabulary: `Proposed` (unresolved, no authority), `Accepted` (agreed and relied upon),
   `Provisional` (accepted as the current approach while an open proposal may replace it), `Superseded`.
5. Accepted records are immutable. To change one, write a successor, mark the predecessor
   `Superseded by NNNN` and link the successor in its `Supersedes / superseded by` line. Never rewrite
   history to match the code; if the code and the record disagree, report the divergence.
6. When a decision has an `Conditions to revisit` item and that condition becomes true, the revisiting
   is new work with its own record, not an edit here.
7. Link accepted records from the affected `docs/ai/*` page and from the ODD task table when a task
   implements them.

## Scope of an ADR

Use one for significant architectural trade-offs, product direction and storage or security policy:
the kind of choice an agent must not silently overturn. Routine behavior belongs in ODD specs and
tests. Routine use of an existing pattern needs no new record.

Machine-checkable implementation of these records does not exist; they are documentation, and the
commands cited in them are the only verification.
