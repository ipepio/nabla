/**
 * Brand assets: every image path the UI shows comes from this module so that
 * re-branding means dropping a file in `public/`, never editing a component
 * (ADR 0009, spec S3 of `odd/tasks/chat-shell-and-theme-tokens.md`).
 *
 * `mark`/`logo`/`wordmark` are the light-on-dark variants used by the default
 * dark theme; the `*Light` siblings are the dark-on-light variants.
 */
export const BRAND = {
  mark: "/logo_white.png", // squared mark, dark theme
  markLight: "/logo.png", // squared mark, light theme
  logo: "/logo_texto_white.png", // mark + wordmark, dark theme
  logoLight: "/logo_texto.png", // mark + wordmark, light theme
  wordmark: "/text_white.png", // wordmark only, dark theme
  wordmarkLight: "/texto.png", // wordmark only, light theme
} as const;

/** A brand asset kind: `mark`, `logo`, `wordmark` and their light siblings. */
export type BrandAsset = keyof typeof BRAND;

/** Absolute URL (Vite base) of one brand asset. */
export function brandAsset(kind: BrandAsset): string {
  return BRAND[kind];
}
