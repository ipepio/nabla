// Optional hero copy for the empty chat (S1). The empty state shows one brand
// image and, under it, whatever this module resolves.
//
// Values come from the local env files (`VITE_HERO_TITLE`, `VITE_HERO_SUBTITLE`)
// and are empty by default: with no configuration the empty state stays as just
// the logo + chips + composer. Like the colour palette (ADR 0009) these are
// baked at build time, so changing them is a rebuild, not a runtime setting.
//
// Tests and tooling can read HERO_KEYS to know the supported names.

const raw = import.meta.env as Record<string, string | undefined>;

function read(key: string): string {
  return (raw[key] ?? "").trim();
}

export const HERO = {
  /** Main line under the logo; empty hides the element entirely. */
  title: read("VITE_HERO_TITLE"),
  /** Secondary line under the title; empty hides the element entirely. */
  subtitle: read("VITE_HERO_SUBTITLE"),
} as const;
