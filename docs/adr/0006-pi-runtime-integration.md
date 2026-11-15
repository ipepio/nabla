# 0006 — Pi runtime integration (provisional)

- Status: Provisional
- Scope: Architecture / runtime
- Acceptance evidence: ODD `odd/tasks/pi-runtime.md` `S1`–`S5` ("pi tiene que estar dentro de tauri,
  no tiene que ir a parte y correr en el ordenador"; RPC-first with `--mode rpc`; one process per
  session; start/stop/restart with crash-only retry; cross-platform kill), resting on `D-003`.
- Supersedes / superseded by: none. The open proposals `T-001` (SDK versus CLI/RPC, distribution and
  process model) and `T-002` (bundling the runtime in the installers) may replace this record.

## Context

Pi is a Node program (`D-003`). The webview cannot own a process, and the app must not require the
user to start Pi separately. The reference projects show the failure modes to avoid: a Unix-only kill
path (Stark's `process_manager.rs:409`), no request/response correlation, and retries that mask
timeouts as crashes. The integration level (embedded SDK versus CLI over RPC) is open, so the
direction is recorded as revisitable.

## Decision

Integrate Pi as a host-owned, headless child process:

- **RPC-first.** Spawn `settings.piBinary` with `--mode rpc` and speak NDJSON over stdin/stdout. The
  wire codec is `src-tauri/src/rpc/mod.rs` (`FrameCodec`), request/response correlation by request id.
- **The Tauri host owns and supervises it** from `src-tauri/src/process/mod.rs` (`ProcessManager`).
  Nothing about the process is visible to the user; the webview never spawns or signals anything.
- **One process per session**, keyed by the session index row, matching the index in
  `nabla.db` from [ADR 0004](0004-persistence-ownership.md).
- **Lifecycle:** `Spawning → Ready` behind a correlated probe, `stop`/`restart`, crash detection with
  exactly one retry and only for a crash (never for a timeout), and a synthetic `process_exit` event
  carrying the stderr tail.
- **Kill on exit, cross-platform:** process-group kill on Unix (`libc`), `taskkill /T` on Windows,
  with a grace period before the force kill. Today it runs through `stop_all` in
  `Drop for ProcessManager`, so it depends on orderly app teardown.
- **One event bridge:** the manager emits Pi events as the typed `agent-event` envelope
  (`{ sessionId, event }`) from `src-tauri/src/lib.rs`; TypeScript mirrors it in `src/contracts/`.

## Alternatives

Embedding Pi in-process or in the webview is rejected: it would give the renderer process control and
remove the ability to kill the agent with the app. A long-lived shared Pi server for all sessions was
rejected by `S3`: one session, one process keeps a crash local. The SDK path (a sidecar or embedded
library) stays a legitimate future option, which is why this record is provisional.

## Consequences

- A Pi upgrade is a binary/CLI concern, not a Rust ABI concern, while `T-001` remains open.
- Every capability needs an RPC command plus a typed event; ad-hoc side channels are not allowed.
- Supervision is real platform work, so its tests use a fake-pi script emitting known NDJSON, never a
  network call and never a real model.
- Orphan risk remains if the host dies without running destructors; `T-002` and packaging must address
  it before distribution.

## Conditions to revisit

- `T-001` being resolved (SDK sidecar versus CLI/RPC) replaces this record with a successor.
- `T-002` (bundling the runtime) decides how `settings.piBinary` is satisfied on a fresh install; the
  current default points at a `pi` on `PATH`.
- If orphaned children are observed in practice, kill-on-exit needs an OS-level guarantee, not only a
  destructor.
