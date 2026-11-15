/**
 * Brand assets: every image path the UI shows comes from this module so that
 * re-branding means dropping a file in `public/`, never editing a component
 * (ADR 0009, spec S3 of `odd/tasks/chat-shell-and-theme-tokens.md`).
 *
 * `mark`/`logo`/`wordmark` are the light-on-dark variants used by the default
 * dark theme; the `*Light` siblings are the dark-on-light variants.
 *
 * The wordmark entries point at the TRIMMED exports (`*_trimmed.png`): the
 * originals are square 1254x1254 canvases with a wide transparent margin, so
 * sizing them by height only scaled the padding and the logotype stayed tiny.
 * `scripts/trim_png.py <input> <output>` regenerates a trimmed copy when a new
 * export lands; the originals stay in `public/` untouched.
 */
export const BRAND = {
  mark: "/logo_white.png", // squared mark, dark theme
  markLight: "/logo.png", // squared mark, light theme
  logo: "/logo_texto_white.png", // mark + wordmark, dark theme
  logoLight: "/logo_texto.png", // mark + wordmark, light theme
  wordmark: "/text_white_trimmed.png", // wordmark only, dark theme (cropped)
  wordmarkLight: "/texto_trimmed.png", // wordmark only, light theme (cropped)
} as const;

/** A brand asset kind: `mark`, `logo`, `wordmark` and their light siblings. */
export type BrandAsset = keyof typeof BRAND;

/** The base kinds whose light sibling is picked by theme. */
export type BrandBase = "mark" | "logo" | "wordmark";

/** Absolute URL (Vite base) of one brand asset. */
export function brandAsset(kind: BrandAsset): string {
  return BRAND[kind];
}

/**
 * Theme-aware asset: the white-on-dark variant for the dark palette and the
 * black-on-light sibling otherwise, so switching the theme switches the logo
 * too instead of leaving a white mark on a light background.
 */
export function themedBrand(kind: BrandBase, isDark: boolean): string {
  const dark = BRAND[kind];
  if (isDark) return dark;
  return BRAND[`${kind}Light` as BrandAsset];
}
