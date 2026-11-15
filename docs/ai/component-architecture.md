# Component architecture — Vue layer

Rules for new or modified code in `src/`. The current layer is deliberately small: one shell view and
one contracts module. Do not add layers that the current screens do not need.

## Responsibility boundaries

| Kind | Example / path | Owns | Must not own |
| --- | --- | --- | --- |
| Shell view | `src/App.vue` | Screen composition and the user flow of the shell | Transport details, DTO mapping, process lifecycle |
| Domain component | `src/<domain>/components/` (when created) | One coherent interaction | Unrelated domains, hidden global side effects |
| Shared UI | Vuetify (`v-card`, `v-list`, `v-row`) | Presentation and interaction contract | Domain state, command calls |
| Composable | `src/<domain>/` (when created) | A cohesive reactive responsibility and its cleanup | A grab bag of helpers |
| Contracts module | `src/contracts/commands.ts` | The only IPC channel and error normalization | Navigation, dialogs, form state |
| Contracts types | `src/contracts/types.ts` | Mirrors of Rust DTOs | Logic, defaults, derived values |

## The IPC channel

- `src/contracts/commands.ts` is the **only** place that imports `@tauri-apps/api/core` and calls
  `invoke()`. Components import functions from it and nothing else.
- `src/contracts/types.ts` mirrors the Rust structs field by field in camelCase. When a payload
  changes, the Rust DTO changes first and this file follows in the same change.
- Bridge rejections are plain strings today: always render them through `commandErrorMessage()` so
  the UI never shows `[object Object]`. Typed error payloads are a pending gap, not a rule.
- No component knows a command name as a string; each one has a typed wrapper.

## Vuetify and the shell

- Vuetify 4 with the dark theme as default (`src/main.ts`). Keep one UI system; do not add another
  component library or a CSS framework.
- `src/main.ts` installs the Vuetify plugin without a component registry, so each SFC imports the
  components it uses from `vuetify/components`. Keep that convention.
- Build screens from cards: `v-card` + `v-card-title` + `v-card-text`, `v-row`/`v-col` for layout,
  `v-alert` for errors and `v-snackbar` for confirmations. Match the established spacing classes.
- Accessibility belongs to the component: every icon-only `v-btn` carries an `aria-label`.
- CSP stays enabled; inline styles are allowed for Vuetify, remote assets are not.

## State ownership

- Local `ref` first. Use a store only when state must be shared or must survive a screen change; no
  store exists today, and adding one (Pinia or otherwise) needs a concrete need and approval.
- Keep one loading flag per operation (`workspacesLoading`, `adding`, `savingSettings`) so independent
  actions never block each other.
- Model absent, loading, failed and successful states explicitly. Errors are `string | null` rendered
  in a closable `v-alert`; success is transient feedback, never a silent state change.
- Never mutate a prop or a caller-owned object; emit the intent to the owner instead.
- After a successful write, reload or patch the affected data deliberately — do not assume the
  database changed the UI state.

## Extraction

- Extract when a block is a coherent interaction with clear inputs and outputs, not because a file
  grew long. A 300-line shell view with two panels is acceptable; two files with one panel each whose
  contracts are unclear is not.
- Promote to shared UI only when the contract is independent of the domain and a real reuse case
  exists. A wrapper must add policy; do not add pass-through components for symmetry.
- Keep pure transformations as plain functions; use `computed` for derived values and `watch` only
  for effects.

## Review checklist

Identify each changed component's category, state owner, inputs/outputs and the UI states it must
handle (see [verification](verification.md)). Check keyboard operation, labels and focus when you
touch an interactive control. `npm run build` proves types and bundling, not that the screen works.
