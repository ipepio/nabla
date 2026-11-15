# Project status and evidence

Observed state of the repository. Anything not verified here is unknown, not “probably fine”.
Product definition lives in the local `.docs/`; this file tracks the versioned codebase.

## Baseline

| Area | Status | Evidence |
| --- | --- | --- |
| Foundation (shell + store) | Done | `odd/tasks/nabla-foundation.md` T1–T8 `done`, commits `99badca`, `2ff6848`, `a052b57`, `5380f84` |
| Rust tests | Passing (12) | `cargo test` in `src-tauri`: 12 passed, 0 failed |
| Rust lint | Passing | `cargo clippy` in `src-tauri`: no warnings |
| Frontend gate | Passing | `npm run build`: `vue-tsc --noEmit` + `vite build` succeed |
| pi runtime (headless spawn) | Not implemented | `odd/tasks/pi-runtime.md`: R1–R5 all `pending` |
| Frontend tests | Absent | No runner in `package.json`, no test script, no `vitest` |
| GUI acceptance | Not automated | Opening the window is manual (`npm run tauri dev`) |
| CI | Absent | No workflow files in the repository |

## Findings

- **F1 — Command errors are untyped strings.** *Classification:* accepted limitation.
  *Evidence:* `src-tauri/src/commands.rs` returns `Result<T, String>` via `display_err`, documented as
  temporary. *Impact:* the UI can only render a message, not branch on an error kind. *Status:*
  accepted; typed payloads are planned with the runtime feature. *Verification:* reasoned from source;
  no test covers error shapes.
- **F2 — No frontend test runner.** *Classification:* gap. *Evidence:* `package.json` has `dev`,
  `build`, `preview`, `tauri` only. *Impact:* Vue behavior is covered only by `vue-tsc` and manual
  checks. *Status:* open; adding a runner is an approved-decision item, not a drive-by change.
- **F3 — Clippy warning under `--all-targets`.** *Classification:* cosmetic. *Evidence:*
  `clippy::field_reassign_with_default` in the `src-tauri/src/settings.rs` test module; plain
  `cargo clippy` is clean. *Impact:* none on runtime behavior. *Status:* open, low priority.
- **F4 — Session index write path is unused.** *Classification:* expected. *Evidence:*
  `SessionNew` and `upsert_session` carry `#[allow(dead_code)]`. *Impact:* sessions cannot be indexed
  until the runtime spawns pi. *Status:* resolved by R3 in `odd/tasks/pi-runtime.md`.
- **F5 — Cosmetic scaffold leftovers.** *Classification:* cosmetic. *Evidence:*
  `src-tauri/Cargo.toml` still has the template `description`/`authors`, `README.md` is the Tauri+Vue
  template text, `src/assets/vue.svg` is unused by the shell. *Impact:* misdescribes the project to a
  reader. *Status:* open; fix in an explicitly scoped unit.

Do not treat an open finding as permission to fix it.

## Current task / continuation

- Delivered: agent documentation architecture A1–A2 (`AGENTS.md`, `CONTENTS.md`, `docs/ai/*`) per
  `odd/tasks/agents-architecture.md`.
- Pending there: A3 (`docs/adr/` + `docs/templates/`) and A4 (consistency review). Neither folder
  exists yet; do not reference it as if it did.
- Next feature: `odd/tasks/pi-runtime.md` (R1 codec → R2 process manager → R3 commands/events →
  R4 contracts + chat → R5 verification). The document describes a stacked `feat/pi-runtime` branch;
  at the time of writing the work is observed on `feat/nabla-foundation` and that branch does not
  exist yet.

## Maintenance

Refresh this file when a baseline row stops being true. Keep evidence paths and exact command output
short; replace stale checklists with observed results. A row is not proof that a check ran today.
