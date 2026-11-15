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

## Theming and branding

Colors and brand images are configuration, never literals in a component (ADR 0009, specs S3–S5 of
`odd/tasks/chat-shell-and-theme-tokens.md`).

- **Tokens come from `.env`.** `.env.example` is the versioned reference; copy it to `.env` (ignored)
  and edit the hex values. The keys are the contract: `VITE_COLOR_PRIMARY`, `VITE_COLOR_PRIMARY_FOREGROUND`,
  `VITE_COLOR_SECONDARY`, `VITE_COLOR_SECONDARY_FOREGROUND`, `VITE_COLOR_TERTIARY`,
  `VITE_COLOR_TERTIARY_FOREGROUND`, `VITE_COLOR_ACCENT`, `VITE_COLOR_ACCENT_FOREGROUND`,
  `VITE_COLOR_BACKGROUND`, `VITE_COLOR_FOREGROUND`, `VITE_COLOR_MUTED`, `VITE_COLOR_MUTED_FOREGROUND`,
  `VITE_COLOR_DESTRUCTIVE`, `VITE_COLOR_SUCCESS`, `VITE_COLOR_WARNING`, `VITE_COLOR_BORDER`,
  `VITE_COLOR_RING`, `VITE_COLOR_SIDEBAR`, `VITE_COLOR_SIDEBAR_FOREGROUND` and `VITE_RADIUS`
  (a length: `0.625rem`, not a color).
- **`src/lib/theme.ts` is the only reader.** It validates the values (hex, `rgb()`, `hsl()`, `oklch()`),
  falls back to the documented default when a key is missing or unparsable, and writes the CSS custom
  properties on `<html>` at boot. `THEME_VARS` is the env-key → CSS-property mapping. The module is
  framework-free; importing it is enough to apply the palette.
- **Components use Tailwind utilities, never colors.** `bg-background`, `text-muted-foreground`,
  `bg-primary`, `text-success`, `bg-tertiary` and the rest map to the tokens in
  `src/assets/index.css` (`@theme inline`). A literal `#…`, `rgb(…)` or `text-[#…]` in a `.vue` file is
  a defect. `.env` and the CSS blocks are the only places a color value appears.
- **Brand images come from `src/lib/brand.ts`.** `brandAsset("mark")` resolves the path; the `mark`,
  `logo` and `wordmark` kinds are the white-on-dark variants, their `*Light` siblings the dark-on-light
  ones. Never write `/logo.png` or any other asset path in a component, and never inline `src`
  attributes pointing at `public/`.
- **Re-branding requires a rebuild.** Vite bakes `import.meta.env` into the bundle, so editing `.env`
  is picked up by `npm run dev`/`npm run build`, not by the packaged app at runtime. Runtime theming
  from `settings.json` is a future evolution of the same module; see ADR 0009.

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
