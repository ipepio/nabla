# 0005 — SQLite store implementation

- Status: Accepted
- Scope: Persistence / implementation
- Acceptance evidence: `D-009` (confirmed during the `nabla-foundation` implementation).
- Supersedes / superseded by: none

## Context

[ADR 0004](0004-persistence-ownership.md) gives the application its own SQLite database. That leaves
implementation choices that decide whether the file ships and migrates safely on three platforms: the
driver, the migration mechanism, the journal mode and the handling of foreign keys. Nabla ships as a
desktop bundle, so "whatever SQLite the machine has" is not a reliable assumption.

## Decision

Implement the store in `src-tauri/src/store.rs` with:

- **`rusqlite` with the `bundled` feature** (`src-tauri/Cargo.toml`): SQLite is compiled into the
  binary, so no system SQLite version or development package is required on macOS, Linux or Windows.
- **Migrations through `PRAGMA user_version`.** `Store::open` opens the connection, sets pragmas and
  calls `migrate`, which reads the current `user_version` and applies each numbered migration once.
  Version 1 creates `workspaces` and `sessions` with an index on `(workspace_id, updated_at DESC)`.
- **WAL journal mode** (`PRAGMA journal_mode = WAL`) so reads do not block the single writer.
- **Foreign keys enforced explicitly** (`PRAGMA foreign_keys = ON`) with
  `ON DELETE CASCADE` from `sessions` to `workspaces`: deleting a workspace removes its index rows.
- **One connection behind a `Mutex`**, because Tauri commands can arrive concurrently; a poisoned
  mutex surfaces as an error rather than being silently reused.

## Alternatives

The system SQLite (no `bundled` feature) would shrink the binary but makes behavior depend on the
user's OS version. An ORM or query builder adds an abstraction without removing the need for SQL and
migrations. A migration crate (for example `refinery`) is more machinery than a `user_version` counter
needs for a schema this small. An in-process connection pool is unnecessary while one connection plus
a mutex satisfies the access pattern; it becomes worth revisiting only with concurrent long reads.

## Consequences

- The binary carries SQLite; compilation is slower and the release binary larger.
- Every future schema change adds the next `user_version` migration; applied migrations are never
  edited, because existing installs have already run them.
- Tests use `TempDb` scratch directories created by the test (`cargo test` inside `src-tauri`) and
  cover migrations, cascade behavior, uniqueness and reopen.

## Conditions to revisit

- When two processes need the store, WAL alone stops being the answer; that is a new architecture
  decision, not a pragma tweak.
- If session index search or growth makes a single file insufficient, revisit the engine with its own
  ADR and keep the ownership split of [ADR 0004](0004-persistence-ownership.md) intact.
