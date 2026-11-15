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

## Consequences

- Re-branding is configuration plus a rebuild: Vite bakes `import.meta.env` into the bundle, so
  editing `.env` requires `npm run dev`/`npm run build`, and the packaged app does not re-theme at
  runtime. `L4` records this honestly.
- Values are build-time only and single-palette: one env value per token overrides both the light
  and dark CSS blocks, so per-theme env keys (or runtime theming from `settings.json`) require a
  successor decision.
- A missing or unparsable key degrades to the documented default instead of painting an invalid
  declaration, so `.env` mistakes are visible but not fatal.
- Component code cannot express a one-off color; a genuinely new color needs a new token and a
  decision here.
- Brand asset paths live in one module, so replacing a file in `public/` re-brands the UI with no code
  edit; a missing file still surfaces as a broken image at runtime, because no bundler check exists.

## Conditions to revisit

- When `.env` proves insufficient because the maintainer wants to switch palettes without rebuilding:
  a successor record introduces runtime theming from `settings.json` and demotes `.env` to defaults.
- When per-theme values are needed (a light palette that is not a mechanical inversion): the env key
  set grows a light/dark dimension.
- If a design system requires palettes beyond semantic tokens (charts, per-workspace accents), decide
  where those values live rather than adding them ad hoc.

## Verification

`npm run build` (frontend gate) and the presence of `--tertiary`/`--success` in the built CSS under
`dist/`. Documentation only beyond that: no test asserts that a component avoided a literal color.
