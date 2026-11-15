# Agent working contract — Nabla

Nabla is a desktop harness built on Pi (`@earendil-works/pi-coding-agent`): a Tauri v2 host in Rust,
a Vue 3 + shadcn-vue (Tailwind v4) webview, and a headless pi child process per session that the host owns, supervises
and kills with the app. This file is the entry point for any agent working here.

## Start here

1. Read this file, then [CONTENTS.md](CONTENTS.md) to pick the deep-dives your task needs.
2. Read [maintainer preferences](docs/ai/preferences.md) — language, tone, autonomy, git habits.
3. If the task belongs to an ODD feature, read `odd/tasks/<feature>.md` in full before editing.
   Its `## Specs` are authoritative; quoted user statements are exact and must not be paraphrased.
4. Inspect source and configuration before trusting documentation. If they disagree, the code is
   right and the document is a defect to report.

## Where things live

| Path | Owns |
| --- | --- |
| `src/App.vue`, `src/main.ts` | Vue shell: app bar, workspaces panel, settings panel |
| `src/contracts/commands.ts` | The only IPC channel: typed wrappers over Tauri commands |
| `src/contracts/types.ts` | TypeScript mirrors of the Rust DTOs (camelCase) |
| `src-tauri/src/lib.rs` | Tauri builder, state wiring, command registration |
| `src-tauri/src/commands.rs` | Command layer — the only seam between webview and Rust |
| `src-tauri/src/store.rs` | SQLite store (`nabla.db`): workspaces + session index, metadata only |
| `src-tauri/src/settings.rs` | `settings.json`: human-editable preferences, atomic write |
| `src/lib/theme.ts` | Semantic color tokens: reads `VITE_COLOR_*`/`VITE_RADIUS` from `.env` and applies the CSS variables at boot |
| `src/lib/brand.ts` | The single brand module: every logo and wordmark path the UI shows |
| `odd/tasks/` | ODD feature documents (specs, task tables, logs) — orchestrator-owned |
| `docs/` | Versioned agent documentation (this architecture) |
| `.docs/` | Local product definition, Spanish, gitignored (D-004). Never commit it |

## Non-negotiable rules

1. **The contracts seam.** Vue never talks to pi, the filesystem or SQLite directly. Every read and
   write crosses a command in `src-tauri/src/commands.rs`, exposed through `src/contracts/commands.ts`.
   No `invoke()` call and no `@tauri-apps/api/core` import outside `src/contracts/`.
2. **pi owns sessions; the app owns an index.** Transcripts live in pi's JSONL files (`nabla.db` only
   stores `pi_session_path` and metadata). Never copy conversation content into the database.
3. **The app database is a rebuildable projection.** On conflict with pi's files, the files win; the
   index must be reconstructable by scanning them.
4. **Storage ownership.** Files for what the human configures (`settings.json`), SQLite for what the
   app accumulates. Agent memory is a future capability with its own store behind the contracts seam
   (Engram is the pattern reference). A shared database is never the integration point.
5. **Contract shape flows Rust → TS, never back.** Payloads serialize camelCase
   (`#[serde(rename_all = "camelCase")]`) and are mirrored field by field in `src/contracts/types.ts`.
6. **CSP stays enabled** in `src-tauri/tauri.conf.json`. `style-src 'unsafe-inline'` is currently
   required by the Tailwind v4 dev pipeline (HMR style injection); revisit when packaging (ADR 0007).
   `"csp": null` is a known defect in reference projects and is not acceptable here.
7. **Roadmap order:** general desktop assistant → agent coordination → development environment
   (P-001). Do not build a later stage early because a reference project has it.
8. **Scope respect.** `.docs/` is local-only and `odd/` is orchestrator-owned; do not edit either
   unless the task explicitly allows it.
9. **No hardcoded colors or brand asset paths in components.** Colors come from `.env`
   (`VITE_COLOR_*` dark, `VITE_COLOR_LIGHT_*` light), brand images from `src/lib/brand.ts`. See ADR 0009.

## Verify

Exact commands; report observed output, never an intention.

| Purpose | Command | Notes |
| --- | --- | --- |
| Frontend gate | `npm run build` | `vue-tsc --noEmit` + `vite build` |
| Desktop app | `npm run tauri dev` | Opens a window; GUI acceptance stays manual |
| Rust tests | `cargo test` | Run inside `src-tauri`; in-module `#[cfg(test)]` tests |
| Rust lint | `cargo clippy` | Run inside `src-tauri`; `--all-targets` currently adds one test-only warning |

If `cargo` is not found, load the toolchain first: `source "$HOME/.cargo/env"`.

## Branch, commit and slice etiquette

- Conventional Commits: `feat:`, `fix:`, `docs:`, `chore:`, imperative subject, one work unit each.
- Target about 400 authored lines per commit; measurement excludes generated or vendored files.
- Tests and the docs that describe changed behavior travel in the same commit as the code.
- One branch per feature, stacked on the previous one. Never commit `dist/`, `src-tauri/target/`,
  `.docs/`, or anything under a tool cache.
- Never commit, push, rebase or open a pull request without explicit permission from the maintainer.

## Tracking work

ODD documents in `odd/tasks/` are the tracking convention: `## Specs` (`S#` quoted requirements),
a `## Tasks` table with IDs, specs and commit per task, and a `## Log` of `L#` entries recording
decisions and corrections. Keep them current; never rewrite a spec to match what you built.

Runtime feature in progress: [odd/tasks/pi-runtime.md](odd/tasks/pi-runtime.md) — headless pi spawn, NDJSON
RPC, event bridge. Its tasks R1–R5 are still pending, so no pi process is spawned in code today.
Foundation work shipped in [odd/tasks/nabla-foundation.md](odd/tasks/nabla-foundation.md).
