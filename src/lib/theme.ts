/**
 * Theme: one module owns the palette *and* the mode (ADR 0009, spec S1/S2/S4 of
 * `odd/tasks/theme-modes-and-resizable-drawer.md`).
 *
 * The app has two palettes, each configured from its own env keys:
 *
 * - **Dark** (the app default, `.dark` on `<html>` in `index.html`):
 *   `VITE_COLOR_*` — the original key set.
 * - **Light**: `VITE_COLOR_LIGHT_*` — the same suffixes as the dark keys.
 * - **`VITE_RADIUS`** is shared geometry, not a color, so it has one key.
 *
 * The values are not written as inline custom properties on `<html>`: inline
 * declarations would win in both modes and the light palette could never take
 * effect. Instead this module injects one `<style id="nabla-theme">` with two
 * rule blocks, `:root` (light) and `.dark` (dark), so each mode resolves its own
 * values through the normal cascade.
 *
 * Only keys whose env value exists and parses are declared. Anything missing
 * falls through to the `:root`/`.dark` defaults already in
 * `src/assets/index.css`, which stays the CSS fallback (and keeps the theme
 * correct before this module runs). A second hardcoded palette is deliberately
 * absent: one copy of each default lives in the CSS.
 *
 * Framework-free on purpose: importing the module applies the palettes, so the
 * CSS variables are in place before the first render.
 */

/** Dark env key → CSS custom property. `THEME_VARS` is the env-key contract. */
export const THEME_VARS = {
  VITE_COLOR_PRIMARY: "--primary",
  VITE_COLOR_PRIMARY_FOREGROUND: "--primary-foreground",
  VITE_COLOR_SECONDARY: "--secondary",
  VITE_COLOR_SECONDARY_FOREGROUND: "--secondary-foreground",
  VITE_COLOR_TERTIARY: "--tertiary",
  VITE_COLOR_TERTIARY_FOREGROUND: "--tertiary-foreground",
  VITE_COLOR_ACCENT: "--accent",
  VITE_COLOR_ACCENT_FOREGROUND: "--accent-foreground",
  VITE_COLOR_BACKGROUND: "--background",
  VITE_COLOR_FOREGROUND: "--foreground",
  VITE_COLOR_CARD: "--card",
  VITE_COLOR_CARD_FOREGROUND: "--card-foreground",
  VITE_COLOR_MUTED: "--muted",
  VITE_COLOR_MUTED_FOREGROUND: "--muted-foreground",
  VITE_COLOR_DESTRUCTIVE: "--destructive",
  VITE_COLOR_SUCCESS: "--success",
  VITE_COLOR_WARNING: "--warning",
  VITE_COLOR_BORDER: "--border",
  VITE_COLOR_RING: "--ring",
  VITE_COLOR_SIDEBAR: "--sidebar",
  VITE_COLOR_SIDEBAR_FOREGROUND: "--sidebar-foreground",
  VITE_COLOR_SIDEBAR_ACCENT: "--sidebar-accent",
  VITE_COLOR_SIDEBAR_ACCENT_FOREGROUND: "--sidebar-accent-foreground",
  VITE_COLOR_SIDEBAR_BORDER: "--sidebar-border",
  VITE_RADIUS: "--radius",
} as const;

/** A CSS custom property name this module declares (for example `--tertiary`). */
export type ThemeVarName = (typeof THEME_VARS)[keyof typeof THEME_VARS];

/** An env key this module reads (for example `VITE_COLOR_TERTIARY`). */
export type ThemeEnvKey = keyof typeof THEME_VARS;

/** The three persisted modes (`settings.json` `theme`) and the runtime contract. */
export type ThemeMode = "dark" | "light" | "system";

/** The id of the single `<style>` element this module owns. */
const STYLE_ELEMENT_ID = "nabla-theme";

/** Shared length token: one value for both palettes, declared once on `:root`. */
const RADIUS_KEY: ThemeEnvKey = "VITE_RADIUS";

/**
 * Loose syntax check, not a color parser: hex, `rgb()`/`hsl()` and the
 * `oklch()` family the token files already use. Anything else is dropped, which
 * keeps a typo in the env file from painting an invalid declaration over the
 * CSS fallback.
 */
const COLOR_PATTERN =
  /^(?:#[0-9a-f]{3,8}|(?:rgb|rgba|hsl|hsla|oklch|oklab)\([^()]*\))$/i;

/** `--radius` is a length, not a color: unit-less zero or a px/rem/em value. */
const LENGTH_PATTERN = /^(?:0|[0-9]*\.?[0-9]+(?:px|rem|em|%))$/;

/** The light twin of a dark color key: `VITE_COLOR_X` → `VITE_COLOR_LIGHT_X`. */
function lightEnvKey(key: ThemeEnvKey): string {
  return key.replace(/^VITE_COLOR_/, "VITE_COLOR_LIGHT_");
}

/**
 * Env value for one key, or `null` when it is absent, blank or unparsable. A
 * `null` result means "declare nothing": the CSS fallback in
 * `src/assets/index.css` keeps that token.
 */
function envValue(key: string): string | null {
  // Env values are untyped at the boundary: read as `unknown` instead of
  // letting `any` leak into the palette.
  const raw: unknown = (import.meta.env as Record<string, unknown>)[key];
  if (typeof raw !== "string") return null;
  const value = raw.trim();
  if (value === "") return null;
  const pattern = key === RADIUS_KEY ? LENGTH_PATTERN : COLOR_PATTERN;
  return pattern.test(value) ? value : null;
}

/** The declarations of one mode, as CSS text; empty when no key is configured. */
function declarations(scheme: "dark" | "light"): string {
  const lines: string[] = [];
  for (const key of Object.keys(THEME_VARS) as ThemeEnvKey[]) {
    if (key === RADIUS_KEY) continue;
    const value = envValue(scheme === "light" ? lightEnvKey(key) : key);
    if (value !== null) lines.push(`${THEME_VARS[key]}: ${value};`);
  }
  if (scheme === "light") {
    // Geometry is mode-independent, so it belongs to the `:root` block.
    const radius = envValue(RADIUS_KEY);
    if (radius !== null) lines.push(`${THEME_VARS.VITE_RADIUS}: ${radius};`);
  }
  return lines.join(" ");
}

/**
 * The stylesheet this module injects: `:root` carries the light palette and
 * `.dark` overrides it with the dark one, exactly like the fallback blocks in
 * `src/assets/index.css`.
 */
function themeCss(): string {
  return [
    `:root { ${declarations("light")} }`,
    `.dark { ${declarations("dark")} }`,
  ].join("\n");
}

/** Narrow the persisted `settings.theme` string to one of the three modes. */
export function asThemeMode(value: string | null | undefined): ThemeMode {
  return value === "light" || value === "system" ? value : "dark";
}

/** Does a mode end up on the dark palette right now? `system` asks the OS. */
export function isDarkMode(mode: ThemeMode): boolean {
  if (mode !== "system") return mode === "dark";
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    // No media query support: keep the app default (`index.html` ships `.dark`).
    return true;
  }
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/**
 * Write the two palette blocks into `<head>`, replacing the previous ones.
 *
 * Idempotent: every call leaves exactly one `<style id="nabla-theme">`. It is
 * also removed and re-appended, so the palette rules stay last in `<head>` and
 * win over the equal-specificity `:root`/`.dark` fallbacks in
 * `src/assets/index.css` whatever order the bundler injected them in.
 */
export function applyTheme(): void {
  if (typeof document === "undefined") return;
  const style = document.createElement("style");
  style.id = STYLE_ELEMENT_ID;
  style.textContent = themeCss();
  document.getElementById(STYLE_ELEMENT_ID)?.remove();
  document.head.appendChild(style);
}

/**
 * Put `<html>` on the requested palette and make sure the palette rules are
 * present. `system` resolves `prefers-color-scheme` at call time; the app has no
 * listener for a live OS change, so the next settings load or toggle re-resolves
 * it.
 */
export function setThemeMode(mode: ThemeMode): void {
  if (typeof document === "undefined") return;
  document.documentElement.classList.toggle("dark", isDarkMode(mode));
  applyTheme();
}

// Self-applying side effect: importing the module is enough to declare both
// palettes, so the CSS variables exist before the first render. Guarded so a
// non-DOM tooling context can import the module safely.
if (typeof document !== "undefined") {
  applyTheme();
  // The bundler appends the `index.css` <style> while the module graph is still
  // evaluating, i.e. after the call above. One deferred call puts this module's
  // rules back at the end of `<head>` without re-reading any env value.
  queueMicrotask(applyTheme);
}
