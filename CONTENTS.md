# Repository navigation

Start with [AGENTS.md](AGENTS.md). This index separates durable working rules from project facts, so
an agent can load only what its task needs.

## Reading map

| File | Purpose | Read when |
| --- | --- | --- |
| [docs/ai/preferences.md](docs/ai/preferences.md) | Confirmed maintainer preferences and adjustable defaults | Every session |
| [docs/ai/architecture.md](docs/ai/architecture.md) | Process tree, module map, capability placement, accepted rules | Exploring or changing behavior |
| [docs/ai/component-architecture.md](docs/ai/component-architecture.md) | Vue layer: component categories, state ownership, shadcn-vue & performance | UI, component or state changes |
| [docs/ai/conventions.md](docs/ai/conventions.md) | TypeScript, serde boundary and Rust error conventions | Before writing code |
| [docs/ai/verification.md](docs/ai/verification.md) | Exact commands, test-first expectations, known gaps | Before checking a change |
| [docs/ai/workflow.md](docs/ai/workflow.md) | ODD sessions, spec-verbatim work, work-unit commits | Every task |
| [docs/ai/project-status.md](docs/ai/project-status.md) | Current state, findings, continuation point | Starting or resuming work |

## Project map

| Area | Location | Status |
| --- | --- | --- |
| Desktop host | `src-tauri/src/lib.rs` | Working: builder, state, command registration |
| Command seam | `src-tauri/src/commands.rs` | Working: 6 commands, string errors |
| App store (SQLite) | `src-tauri/src/store.rs` | Working: workspaces + session index, 12 tests total with settings |
| Human config | `src-tauri/src/settings.rs` | Working: `settings.json`, atomic write |
| Runtime (pi child) | `src-tauri/src/` | Working: headless pi spawn, NDJSON RPC, event bridge, subagents & MCP |
| Webview | `src/` (Vue 3 + shadcn-vue + Tailwind v4) | Working: sidebar, chat transcript, command palette (`⌘K`), settings dialog, resizable tools drawer |
| IPC contracts | `src/contracts/` | Working: typed wrappers and DTO mirrors |
| Frontend tests | `tests/` (`node --test tests/`) | Working: pure modules and composables via Node's built-in test runner |
| Product definition | `.docs/` | Local, Spanish, gitignored; not part of the repository |

## Where future capabilities land

Plugins, integrations, MCP servers, skills and subagents do not have code folders yet. The intended
home of each capability, and the rule that keeps them out of the core seam, is documented in
[docs/ai/architecture.md](docs/ai/architecture.md#capability-placement). Do not create empty module
folders in advance; create the folder when the capability has code, tests and an owner.

## Provenance

This structure adapts the frontend template of `github.com/ipepio/dev-templates` (root `AGENTS.md`,
`CONTENTS.md` index, `docs/ai/*` deep-dives) to a Tauri v2 + Vue + Rust desktop application. The
template's NestJS backend guidance does not apply. Architectural decision records and the ADR/feature
templates are a later step (`docs/adr/` and `docs/templates/`, tracked as task A3 in
[odd/tasks/agents-architecture.md](odd/tasks/agents-architecture.md)).
