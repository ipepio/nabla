# 0009 — Configurable theme tokens and a single brand module

- Status: Accepted
- Scope: Architecture / presentation configuration
- Acceptance evidence: `S3` — «quiero que las imagenes ... sean todo configuraciones ... las imagenes
  está claro que es cambiar el public»; `S4` — «quiero que definas colores primarios, secundarios
  terciarios de diferentes tipos y que en las .env se cambie el hex de ese color»; `S5` —
  «IMPORTANTE, PARA TODO EL PROYECTO, ACTUALIZA LOS AGENTS.MD SI ES NECESARIO»; all of
  `odd/tasks/chat-shell-and-theme-tokens.md`. Log entries `L2`–`L4` record the white logo variants and
  the build-time limitation.
- Supersedes / superseded by: none

> **Addendum (2026-02, `theme-modes-and-resizable-drawer` T1):** decision 2 below is amended — the
> palette is no longer written as inline styles on `<html>`, and there is now a second, light palette.
> See [Addendum — light palette and rule-block injection](#addendum--light-palette-and-rule-block-injection).

## Context

The chat shell needs a palette and brand imagery, and the maintainer asked for both to be
configuration rather than code: colors edited as hex in `.env`, images replaced in `public/`. Without
a rule, every new component would drift into literal `#…` values and `/logo.png` strings, and
re-branding would mean editing components.

The repository already had shadcn-vue tokens in `src/assets/index.css` with `:root` (light) and
`.dark` blocks, `.dark` on `<html>` by default. It had no `.env`, no `.env` ignore rule and no brand
module; `src/components/AppSidebar.vue` still carried `const logoSource = "/logo.png"` and is owned by
the chat-redesign task, not by this decision.

## Decision

1. **`.env` is the color configuration.** `VITE_COLOR_*` keys (`PRIMARY`, `SECONDARY`, `TERTIARY`,
   `ACCENT`, `BACKGROUND`, `FOREGROUND`, `MUTED`, `DESTRUCTIVE`, `SUCCESS`, `WARNING`, `BORDER`,
   `RING`, `SIDEBAR` plus their `_FOREGROUND` counterparts) carry hex values; `VITE_RADIUS` carries a
   length. `.env.example` is the versioned reference with one comment per key; `.env` is gitignored.
2. **`src/lib/theme.ts` is the only reader.** It validates each value loosely (hex, `rgb()`, `hsl()`,
   `oklch()`), falls back to a documented default when a key is absent or unparsable, and writes the
   CSS custom properties as inline styles on `<html>`, so one env palette wins over both the `:root`
   and `.dark` blocks. Those blocks stay as the CSS fallback. The module is framework-free and
   applies the palette on import.
3. **`src/lib/brand.ts` is the only source of asset paths.** `BRAND` maps the kinds `mark`, `logo`
   and `wordmark` (white-on-dark) and `markLight`, `logoLight`, `wordmarkLight` to files in
   `public/`; `brandAsset(kind)` resolves them.
4. **Components use tokens and the brand module.** A literal color or `/logo*.png` path in a `.vue`
   file is a defect. The rule is stated in `AGENTS.md` (non-negotiable 9),
   `docs/ai/conventions.md` and `docs/ai/component-architecture.md`.

The new semantic tokens `--tertiary`, `--tertiary-foreground`, `--success` and `--warning` are mapped
in the `@theme inline` block, so `bg-tertiary`, `text-success` and similar utilities exist.

## Alternatives

- **Runtime theme editor only (`settings.json`).** Rejected for now: it needs a live CSS-variable
  writer, a picker UI and a persistence round-trip, and the maintainer asked specifically for `.env`
  values.
- **Per-component color modules.** Rejected: it hides the palette in TypeScript constants and defeats
  the single-file re-brand the spec asks for.
- **Keeping colors hardcoded next to each component.** Rejected by `S4`/`S5`.
- **An image-import pipeline (`src/assets` + Vite `?url` imports).** Rejected: `S3` states the image
  replacement happens in `public/`, so paths stay public URLs and no rebuild-time import map is
  needed.

## Addendum — light palette and rule-block injection

Accepted with `odd/tasks/theme-modes-and-resizable-drawer.md` (specs S1, S2, S4). Decision 2 above is
amended, the rest of the record stands:

- **Inline custom properties are gone.** Writing the palette as inline custom properties on `<html>`
  made those declarations win in *both* modes, so the light theme could never render. `src/lib/theme.ts`
  now injects one `<style id="nabla-theme">` into `<head>` with two rule blocks: `:root` carries the
  light values and `.dark` the dark ones. Nothing writes palette properties through
  `documentElement.style` any more.
- **Two env key sets, one per palette.** `VITE_COLOR_*` stays the dark palette (the app default,
  `.dark` in `index.html`). The light palette repeats the same suffixes behind a
  `VITE_COLOR_LIGHT_` prefix (`VITE_COLOR_LIGHT_BACKGROUND`, `VITE_COLOR_LIGHT_PRIMARY`, …,
  `VITE_COLOR_LIGHT_SIDEBAR_FOREGROUND`). `VITE_RADIUS` stays a single shared length in the `:root`
  block, because geometry is not a theme.
- **Precedence is: configured env value → `src/assets/index.css` default.** Only keys whose env value
  exists and parses are declared; a missing or unparsable key emits nothing and that token keeps the
  `:root`/`.dark` default in the CSS, which remains the single copy of the neutral palette. The injected
  element is kept last in `<head>`, so configured values win over the stylesheet regardless of the order
  the bundler injected it in.
- **Mode is runtime, values are build-time.** `setThemeMode("dark" | "light" | "system")` only toggles
  the `dark` class (resolving `system` through `prefers-color-scheme`) and re-injects the palette;
  `applyTheme()` re-injects alone. Switching modes needs no rebuild, changing hex values still needs
  `npm run dev`/`npm run build`.
- **Residual limitation.** The persisted mode is read through the Tauri settings command after the
  first render, so a persisted *light* theme can show one dark frame at startup. Avoiding it needs a
  synchronous pre-paint read of the mode (for example a blocking script in `index.html`), which the app
  does not have; the limitation is documented rather than hidden.

## Consequences

- Re-branding is configuration plus a rebuild: Vite bakes `import.meta.env` into the bundle, so
  editing `.env` requires `npm run dev`/`npm run build`, and the packaged app does not pick up new
  color *values* at runtime (switching *modes* is runtime, see the addendum). `L4` records this
  honestly.
- Values are build-time only: one env value per token per mode, and both palettes share the same
  build. Mode switching is runtime (`settings.json` → `setThemeMode`), changing hex values is not. See
  the addendum above; the single-palette limitation it replaced is history.
- A missing or unparsable key declares nothing and falls through to the `:root`/`.dark` defaults in
  `src/assets/index.css`, so an env mistake degrades to the neutral palette instead of painting an
  invalid declaration.
- Component code cannot express a one-off color; a genuinely new color needs a new token and a
  decision here.
- Brand asset paths live in one module, so replacing a file in `public/` re-brands the UI with no code
  edit; a missing file still surfaces as a broken image at runtime, because no bundler check exists.

## Conditions to revisit

- When `.env` proves insufficient because the maintainer wants to switch palettes without rebuilding:
  a successor record introduces runtime theming from `settings.json` and demotes `.env` to defaults.
- When per-theme values are needed (a light palette that is not a mechanical inversion): the env key
  set grows a light/dark dimension. *Resolved by the addendum above: `VITE_COLOR_LIGHT_*`.*
- If the startup palette flash (see the addendum's residual limitation) becomes unacceptable: add a
  synchronous pre-paint read of the persisted mode before the webview mounts.
- If a design system requires palettes beyond semantic tokens (charts, per-workspace accents), decide
  where those values live rather than adding them ad hoc.

## Verification

`npm run build` (frontend gate) and the presence of `--tertiary`/`--success` in the built CSS under
`dist/`. Documentation only beyond that: no test asserts that a component avoided a literal color.
