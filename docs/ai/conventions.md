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

- **Brand images live in `public/` and are resolved through `src/lib/brand.ts`.** Replace the file
  and the app re-brands: `logo_white.png` (combined mark + wordmark, dark), `logo.png` (light),
  plus the separate `mark`/`wordmark` variants. Never write an image path in a component.
- **Empty-state copy is optional and configured too**: `VITE_HERO_TITLE` and `VITE_HERO_SUBTITLE`
  (`src/lib/hero.ts`). Both empty by default, so the empty state shows only the logo, the chips and
  the composer.

- **Tokens come from `.env`, one palette per mode.** `.env.example` is the versioned reference; copy
  it to `.env` (ignored) and edit the hex values. The dark palette is the app default and owns the
  original keys: `VITE_COLOR_PRIMARY`, `VITE_COLOR_PRIMARY_FOREGROUND`, `VITE_COLOR_SECONDARY`,
  `VITE_COLOR_SECONDARY_FOREGROUND`, `VITE_COLOR_TERTIARY`, `VITE_COLOR_TERTIARY_FOREGROUND`,
  `VITE_COLOR_ACCENT`, `VITE_COLOR_ACCENT_FOREGROUND`, `VITE_COLOR_BACKGROUND`,
  `VITE_COLOR_FOREGROUND`, `VITE_COLOR_MUTED`, `VITE_COLOR_MUTED_FOREGROUND`,
  `VITE_COLOR_DESTRUCTIVE`, `VITE_COLOR_SUCCESS`, `VITE_COLOR_WARNING`, `VITE_COLOR_BORDER`,
  `VITE_COLOR_RING`, `VITE_COLOR_SIDEBAR` and `VITE_COLOR_SIDEBAR_FOREGROUND`. The light palette
  repeats **the same suffixes** behind a `VITE_COLOR_LIGHT_` prefix — `VITE_COLOR_LIGHT_BACKGROUND`,
  `VITE_COLOR_LIGHT_FOREGROUND`, `VITE_COLOR_LIGHT_PRIMARY`, … `VITE_COLOR_LIGHT_SIDEBAR_FOREGROUND`.
  `VITE_RADIUS` is shared geometry, not a color (a length: `0.625rem`), so it has one key.
- **`src/lib/theme.ts` is the only reader.** It validates the values (hex, `rgb()`, `hsl()`, `oklch()`)
  and injects a single `<style id="nabla-theme">` into `<head>` with two rule blocks: `:root` carries
  the light values and `.dark` the dark ones. It never writes inline custom properties on `<html>` —
  those would win in both modes and the light palette could not take effect. `THEME_VARS` is the
  dark env-key → CSS-property mapping; light keys are the same names with the prefix. The module is
  framework-free; importing it is enough to declare both palettes.
- **Missing keys fall through to the CSS.** A key that is absent or unparsable is not declared at all,
  so that token keeps the `:root`/`.dark` default already in `src/assets/index.css`. The defaults live
  there exactly once; there is no second palette in TypeScript. Configured values win because the
  injected element is kept last in `<head>`, after the bundler's stylesheet.
- **The mode is `settings.json`'s `theme`, applied by this module.** `setThemeMode("dark" | "light"
  | "system")` toggles `.dark` on `<html>` (resolving `system` through `prefers-color-scheme`) and
  re-injects the palette; `applyTheme()` only re-injects and is idempotent. `asThemeMode()` narrows the
  persisted string and `isDarkMode()` resolves a mode. No component toggles `.dark` or the palette
  itself: the header button, the settings dialog and the boot path all call `setThemeMode`.
- **Components use Tailwind utilities, never colors.** `bg-background`, `text-muted-foreground`,
  `bg-primary`, `text-success`, `bg-tertiary` and the rest map to the tokens in
  `src/assets/index.css` (`@theme inline`). A literal `#…`, `rgb(…)` or `text-[#…]` in a `.vue` file is
  a defect. `.env` and the CSS blocks are the only places a color value appears.
- **Brand images come from `src/lib/brand.ts`.** `brandAsset("mark")` resolves the path; the `mark`,
  `logo` and `wordmark` kinds are the white-on-dark variants, their `*Light` siblings the dark-on-light
  ones. Never write `/logo.png` or any other asset path in a component, and never inline `src`
  attributes pointing at `public/`.
- **Re-branding requires a rebuild.** Vite bakes `import.meta.env` into the bundle, so editing `.env`
  is picked up by `npm run dev`/`npm run build`, not by the packaged app at runtime. The *mode* switch
  is runtime (it only flips the `.dark` class); the *values* are still build-time. See ADR 0009.

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
