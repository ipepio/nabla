# Implementation conventions

Follow the conventions already in the code. Where a convention conflicts with correctness, safety or
the rules in [AGENTS.md](../../AGENTS.md), report the conflict instead of extending the defect.

## Language and comments

- Repository artifacts (code, comments, UI copy, these documents) are in English. `.docs/` is Spanish
  and local-only; quote it in Spanish when a decision needs its exact wording.
- Module headers state the rule the module protects (see the first lines of `src-tauri/src/store.rs`),
  not a restatement of the code. Comment the non-obvious decision, never the syntax.

## TypeScript (Vue)

- `tsconfig.json` is `strict` with `noUnusedLocals` and `noUnusedParameters`. No `any`, no `@ts-ignore`,
  no disabled checks to silence a real error.
- Type the absent case honestly: `string | null`, not `""`; do not hide nullability with `!` or casts.
- `unknown` at boundaries. Bridge rejections are `unknown` and normalized by `commandErrorMessage()`.
- Async functions used from templates are `void`-called or awaited deliberately; keep loading flags in
  `try/finally` so a failure cannot leave the UI stuck as busy.
- Validate before side effects: `handleAddWorkspace` rejects an empty path before calling the command.
- Imports of Vuetify components are per-SFC (`vuetify/components`), matching `src/App.vue`.

## The serde boundary

- Every payload crossing the bridge is a Rust struct with `#[serde(rename_all = "camelCase")]`.
- TypeScript mirrors it field by field in `src/contracts/types.ts`; optional Rust fields become
  `| null` in TS (not `?`), because serde always emits the key.
- `settings.json` uses the same camelCase convention (`piBinary`, `theme`).
- Change the Rust shape first, then the mirror, in the same work unit. A TS-only payload change is a
  defect even when it type-checks.
- Unknown fields in `settings.json` are tolerated on read; do not tighten that into an error.

## Rust

- One error enum per module (`StoreError`, `SettingsError`) implementing `Display` and
  `std::error::Error`, with `From<rusqlite::Error>` or `From<std::io::Error>` where relevant.
- Commands return `Result<T, String>` today and map errors with `display_err`. Typed error payloads
  arrive with the runtime feature: do not freeze an error shape before then.
- Validate at the command layer (existence, emptiness, uniqueness) and let the store own SQL and
  invariants. The store never receives a path the caller did not check.
- Writes that replace a file are atomic (write to a temporary sibling, then rename). Load whole, save
  whole; no partial-file editing.
- Migrations go through `PRAGMA user_version`; never edit an applied migration. Foreign keys cascade.
- No `unwrap()`/`expect()` on fallible I/O outside the Tauri builder in `src-tauri/src/lib.rs` and
  test code; return the error.
- `#[allow(dead_code)]` requires a comment naming the feature that will use the item.

## Changes

- Keep the change on the requested behavior; put unrelated cleanup in its own work unit.
- Adding or upgrading a dependency needs approval; reuse what the manifests already declare
  (`package.json`, `src-tauri/Cargo.toml`).
- Never run a formatter or lint autofix over files outside your change.
- Update the documentation that describes behavior you changed: `AGENTS.md`, `docs/ai/*` and the ODD
  feature document in `odd/tasks/`.
