/**
 * Theme tokens: `.env` (`VITE_COLOR_*`, `VITE_RADIUS`) is the only source of the
 * application palette. This module validates those values and writes the CSS
 * custom properties once at boot; components never carry literal colors
 * (ADR 0009, specs S4/S5 of `odd/tasks/chat-shell-and-theme-tokens.md`).
 *
 * Applied properties are written as inline custom properties on
 * `<html>`: the env palette is one set of values, so those declarations win over
 * both the `:root` (light) and the `.dark` blocks in `src/assets/index.css`.
 * Those blocks remain the fallback for a missing or invalid key, and they keep
 * the theme correct before this module runs. Per-theme env keys would need a
 * successor decision (ADR 0009).
 *
 * Framework-free on purpose: `src/main.ts` imports this module for its boot
 * side effect, so the palette is applied before the first render.
 */

/** Env key → CSS custom property. The exact contract both workers respect. */
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
  VITE_COLOR_MUTED: "--muted",
  VITE_COLOR_MUTED_FOREGROUND: "--muted-foreground",
  VITE_COLOR_DESTRUCTIVE: "--destructive",
  VITE_COLOR_SUCCESS: "--success",
  VITE_COLOR_WARNING: "--warning",
  VITE_COLOR_BORDER: "--border",
  VITE_COLOR_RING: "--ring",
  VITE_COLOR_SIDEBAR: "--sidebar",
  VITE_COLOR_SIDEBAR_FOREGROUND: "--sidebar-foreground",
  VITE_RADIUS: "--radius",
} as const;

/** A CSS custom property name this module owns (for example `--tertiary`). */
export type ThemeVarName = (typeof THEME_VARS)[keyof typeof THEME_VARS];

/** An env key this module reads (for example `VITE_COLOR_TERTIARY`). */
export type ThemeEnvKey = keyof typeof THEME_VARS;

/**
 * Documented defaults, identical to `.env.example`. They are used when a key is
 * absent or its value does not parse, and they mirror the `:root`/`.dark`
 * declarations in `src/assets/index.css` for the tokens this module owns.
 */
export const THEME_DEFAULTS: Record<ThemeEnvKey, string> = {
  VITE_COLOR_PRIMARY: "#ededed",
  VITE_COLOR_PRIMARY_FOREGROUND: "#0a0a0a",
  VITE_COLOR_SECONDARY: "#3a3a3a",
  VITE_COLOR_SECONDARY_FOREGROUND: "#fafafa",
  VITE_COLOR_TERTIARY: "#60a5fa",
  VITE_COLOR_TERTIARY_FOREGROUND: "#0a0a0a",
  VITE_COLOR_ACCENT: "#262626",
  VITE_COLOR_ACCENT_FOREGROUND: "#fafafa",
  VITE_COLOR_BACKGROUND: "#0a0a0a",
  VITE_COLOR_FOREGROUND: "#fafafa",
  VITE_COLOR_MUTED: "#262626",
  VITE_COLOR_MUTED_FOREGROUND: "#a3a3a3",
  VITE_COLOR_DESTRUCTIVE: "#f87171",
  VITE_COLOR_SUCCESS: "#4ade80",
  VITE_COLOR_WARNING: "#fbbf24",
  VITE_COLOR_BORDER: "#262626",
  VITE_COLOR_RING: "#737373",
  VITE_COLOR_SIDEBAR: "#171717",
  VITE_COLOR_SIDEBAR_FOREGROUND: "#fafafa",
  VITE_RADIUS: "0.625rem",
};

/**
 * Loose syntax check, not a color parser: hex, `rgb()`/`hsl()` and the
 * `oklch()` family the token files already use. Anything else falls back, which
 * keeps a typo in `.env` from painting an invalid declaration over the palette.
 */
const COLOR_PATTERN =
  /^(?:#[0-9a-f]{3,8}|(?:rgb|rgba|hsl|hsla|oklch|oklab)\([^()]*\))$/i;

/** `--radius` is a length, not a color: unit-less zero or a px/rem/em value. */
const LENGTH_PATTERN = /^(?:0|[0-9]*\.?[0-9]+(?:px|rem|em|%))$/;

/** Resolved value of every property this module owns, keyed by property name. */
export type ResolvedTheme = Record<ThemeVarName, string>;

function isValid(key: ThemeEnvKey, value: string): boolean {
  return (key === "VITE_RADIUS" ? LENGTH_PATTERN : COLOR_PATTERN).test(value);
}

function resolveValue(key: ThemeEnvKey): string {
  // `import.meta.env` values are untyped at the boundary: read as `unknown`
  // instead of letting `any` leak into the palette.
  const raw: unknown = import.meta.env[key];
  const candidate = typeof raw === "string" ? raw.trim() : "";
  if (candidate === "" || !isValid(key, candidate)) {
    return THEME_DEFAULTS[key];
  }
  return candidate;
}

/** Every owned property with its final value, without touching the DOM. */
export function resolveTheme(): ResolvedTheme {
  const resolved = {} as ResolvedTheme;
  for (const key of Object.keys(THEME_VARS) as ThemeEnvKey[]) {
    resolved[THEME_VARS[key]] = resolveValue(key);
  }
  return resolved;
}

/**
 * Write the palette as inline custom properties on `<html>`.
 *
 * One approach, documented: the palette comes from one env set, so the inline
 * declarations on the root element win over both the `:root` (light) and the
 * `.dark` blocks in `src/assets/index.css`. Those blocks stay as the fallback
 * used when a key is absent, and they keep the theme working before this module
 * runs. Per-theme env keys (or runtime theming from `settings.json`) would
 * arrive as a successor decision, not here.
 *
 * @param target Element to write on; defaults to `document.documentElement`.
 * @returns The applied values, useful in tooling and tests.
 */
export function applyTheme(target?: HTMLElement | null): ResolvedTheme {
  const resolved = resolveTheme();
  const element =
    target ?? (typeof document === "undefined" ? null : document.documentElement);
  if (element) {
    for (const [property, value] of Object.entries(resolved)) {
      element.style.setProperty(property, value);
    }
  }
  return resolved;
}

// Self-applying side effect: nothing imports this module yet (`src/main.ts` is
// wired in a follow-up task), and the palette must be in place before the first
// paint. Guarded so a non-DOM tooling context can import the module safely.
if (typeof document !== "undefined") {
  applyTheme();
}
