# Local AGENTS.md template

Copy the content below into a real feature's `AGENTS.md`, then replace the placeholders. This file is
a reference template, not active instructions for a nonexistent feature. The root
[AGENTS.md](../../AGENTS.md) still applies in full; keep only what is specific to the feature.

```markdown
# Feature: <name>

## Ownership

- Owns: <capability and the state it alone writes>.
- Does not own: <neighboring capabilities and the module they live in>.
- Entry seam: <the Tauri command(s) in `src-tauri/src/commands.rs` and the wrapper in
  `src/contracts/commands.ts` that everything crosses>.
- Internal modules other features must not import: <actual paths>.
- Place in the process tree: <webview (`src/`), Rust host (`src-tauri/`), or the pi child process>,
  and why that layer owns it.

## Invariants

- <Rule and the link to its accepted spec (`odd/tasks/<feature>.md` `S#`) or ADR (`docs/adr/NNNN-...`)>.
- <What must not be written where: for example conversation content never enters `nabla.db`, only
  `pi_session_path` and metadata.>
- <Contract direction: a payload change starts in Rust and moves `src/contracts/types.ts` in the same
  change.>
- <Omitted/null/update semantics and compatibility requirement, if any>.

## Existing patterns to follow

- Command + error mapping: `src-tauri/src/commands.rs`.
- Store access and migrations: `src-tauri/src/store.rs`.
- Human-editable settings: `src-tauri/src/settings.rs`.
- Typed contract mirrors: `src/contracts/types.ts`.
- UI state and Vuetify usage: `docs/ai/component-architecture.md`.
- Known local inconsistencies not to copy: <specific exception, or `none`>.

## Verification

- Focused tests: `cargo test <module_or_test_name>` inside `src-tauri`.
- Frontend gate: `npm run build`.
- Rust lint: `cargo clippy` inside `src-tauri`.
- Observable effects that must remain unchanged: <rows, files, event shapes>.
- Manual check, if the change is visible: `npm run tauri dev`.
- Unavailable checks and why: <for example no frontend test runner is installed>.

## Decisions

- Accepted ADRs this feature relies on: <links>.
- Questions requiring the maintainer before behavior changes: <each question and what it blocks>.

Do not repeat global architecture, introduce a local exception without recorded acceptance, and do not
claim a referenced file exists until it does.
```
