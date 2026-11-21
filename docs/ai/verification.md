# Verification

Select checks from the observable effects of the change, not from a fixed ritual. A passing build is
not proof that behavior works, and a checklist is not evidence that a command ran.

## Project commands

Run from the repository root unless the table says otherwise.

| Purpose | Exact command | Notes |
| --- | --- | --- |
| Frontend gate | `npm run build` | `vue-tsc --noEmit` then `vite build`; writes `dist/` (gitignored) |
| Node tests | `node --test tests/` | Pure modules and composables; Node strips the types, no test framework is a dependency |
| Desktop app | `npm run tauri dev` | Starts Vite on port 1420 and opens the window; needs a GUI session |
| Rust tests | `cd src-tauri && cargo test` | In-module `#[cfg(test)]` tests; no network, temp dirs per test |
| Rust lint | `cd src-tauri && cargo clippy` | Clean at the time of writing |
| Rust lint, all targets | `cd src-tauri && cargo clippy --all-targets` | One pre-existing test-only warning (`settings.rs`), see project status F3 |

If `cargo` is missing from `PATH`, load the toolchain with `source "$HOME/.cargo/env"`.
Never install dependencies, add a test runner or upgrade the toolchain on your own initiative.

## Selection matrix

| Change | Applicable evidence |
| --- | --- |
| Documentation only (`AGENTS.md`, `CONTENTS.md`, `docs/`) | Link and path existence, command names, factual claims against source; no build required |
| Rust logic, validation, SQL | `cargo test` with the rule's examples, boundaries and rejected inputs |
| Node-testable module or composable (pure rules, stores) | `node --test tests/` with the rule's examples, boundaries and rejected inputs, plus `npm run build` for the types |
| Command or contract change | Both sides of the seam: Rust tests for behavior, `npm run build` for the TS mirror, payload comparison Rust ↔ TS |
| Vue view / component | `npm run build` plus a manual pass of the affected states in `npm run tauri dev` |
| Process/runtime change | Tests against a fake `pi` script emitting known NDJSON; never a real pi binary or network |
| Config or security surface (CSP, capabilities) | Diff inspection of `src-tauri/tauri.conf.json` and `src-tauri/capabilities/` |

## Test-first expectation

1. **RED** — for behavior changes, add the test that expresses the rule and observe it fail for the
   expected reason. Rust tests go in the module's `#[cfg(test)] mod tests` (see `src-tauri/src/store.rs`)
   and assert through the public interface, including that rejected input leaves stored data unchanged;
   Node-testable modules go in `tests/*.test.ts` (see `tests/composer-images.test.ts`).
2. **GREEN** — implement the minimum change and capture the passing run.
3. **PRESERVE** — for shared code (parsers, helpers, commands), add one test proving an existing
   consumer's behavior still holds.
4. **REFACTOR** — only with the focused tests green.

Justified exceptions: passive documentation, configuration-only edits, or a behavior with no runner.
State the exception and give the proportionate check instead. Do not claim RED/GREEN evidence that
was not observed.

## UI states

For a changed screen, decide explicitly which apply: loading, empty, error/recovery, success,
invalid input, submitting. Explain meaningful exclusions. A window check requires `npm run tauri dev`
and stays manual until GUI automation exists.

## Not covered yet

- Typed command errors: no contract on either side, so only message text can be asserted (F1).
- Frontend component tests: no runner mounts Vue components (F2). The Node suite covers pure modules
  and composables; rendered behavior is checked by `vue-tsc` and manual passes.
- End-to-end desktop automation and CI: absent; nothing enforces these checks automatically.
- Runtime behavior: covered by fake `pi` scripts that emit known NDJSON (see the `FAKE_*_PI` scripts in
  `src-tauri/src/commands.rs`); a real pi binary and the network never run in tests.

## Evidence report

For every executed command report the exact command and the observed result (counts, exit status,
warning text). Classify a failure as introduced, pre-existing or unknown only with evidence. List
skipped or unavailable checks explicitly: an unverified required check means the task is not complete.
