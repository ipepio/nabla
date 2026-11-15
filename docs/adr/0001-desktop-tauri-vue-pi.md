# 0001 — Desktop app on Tauri v2, Vue and a Pi-based harness

- Status: Accepted
- Scope: Architecture
- Acceptance evidence: `D-001`, `D-002`, `D-003` in the local decision register (confirmed by the
  maintainer: desktop application for macOS/Windows/Linux; Tauri v2 and Vue; harness based on Pi).
- Supersedes / superseded by: none

## Context

Nabla is a desktop harness for agent sessions, not a web service. Three separate register entries
describe one product shape and only make sense together: a webview cannot own processes, files or
databases, and Pi is a Node program that must be supervised from outside the webview. Recorded
together, they explain why the repository has exactly the three layers it has.

## Decision

Build one desktop application in three layers:

- A Rust host in `src-tauri/` (Tauri v2) that owns the process tree, the filesystem and SQLite.
- A Vue 3 + Vuetify webview in `src/` that renders state and calls commands; it never talks to pi,
  the filesystem or SQLite directly.
- A headless Pi child process supervised by the host, integrated per
  [ADR 0006](0006-pi-runtime-integration.md).

The webview/host boundary is the command seam: `src-tauri/src/commands.rs` exposes Tauri commands,
`src/contracts/commands.ts` wraps them and `src/contracts/types.ts` mirrors the Rust DTOs field by
field (camelCase). No `@tauri-apps/api/core` import exists outside `src/contracts/`.

## Alternatives

A web application was not chosen: it cannot own local processes and directories. Electron is the
comparable desktop shell but Tauri v2 is what was requested (`D-002`), and its Rust host is where
process supervision and SQLite already live. No alternative was implemented or benchmarked; this
section records the consideration, not a comparison of measurements.

## Consequences

- Rust is the only privileged layer; every new capability needs a command, not a webview shortcut.
- Contract shape flows Rust → TypeScript, never back: a payload change starts in Rust and moves the
  TypeScript mirror in the same change.
- Two toolchains (Node and Cargo) must be present for the frontend gate and the Rust tests.
- Desktop acceptance is manual (`npm run tauri dev`); automated checks cover build, contracts and
  Rust unit tests only.

## Conditions to revisit

- `D-001` leaves minimum OS versions and CPU architectures open; distribution choices belong to the
  open proposals `T-001` (runtime integration) and `T-002` (bundling the runtime in installers).
- `D-002` fixes the stack but not the Vue version policy or complementary tooling.
- `D-003` fixes Pi as the engine but not the integration level, which
  [ADR 0006](0006-pi-runtime-integration.md) holds as provisional.
