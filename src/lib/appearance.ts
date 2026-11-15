/**
 * Appearance state shared by the whole webview (spec S4: one owner per piece of
 * state; ADR 0009: `src/lib/theme.ts` is the only module that *applies* a theme).
 *
 * `theme.ts` is framework-free on purpose, so it cannot hold reactive state.
 * This module is the thin Vue layer on top of it: components read `isDark` to
 * pick the right logo variant and ask `setMode` to change the palette, and every
 * consumer sees the same value because the state lives at module scope.
 *
 * Persistence stays in `settings.json`: callers save the new mode and then call
 * `setMode`, exactly as ChatPanel and AppSidebar already do.
 */
import { computed, ref } from "vue";
import { asThemeMode, isDarkMode, setThemeMode, type ThemeMode } from "@/lib/theme";

/** Live mode; "system" resolves through the media query inside `theme.ts`. */
const mode = ref<ThemeMode>("dark");

/** Resolved appearance, i.e. what the palette is actually showing right now. */
const isDark = computed(() => isDarkMode(mode.value));

/** Applies a mode to the document and republishes it to every consumer. */
function setMode(next: ThemeMode | string): void {
  const normalized = asThemeMode(next);
  mode.value = normalized;
  setThemeMode(normalized);
}

/**
 * Observes the OS scheme so `isDark` keeps telling the truth while the mode is
 * "system": the palette follows the OS, and so must the logo variant.
 */
function followSystemScheme(): void {
  if (typeof window === "undefined" || !window.matchMedia) return;
  const query = window.matchMedia("(prefers-color-scheme: dark)");
  const sync = (): void => {
    if (mode.value !== "system") return;
    setThemeMode("system");
    // `setThemeMode` mutates the DOM; nudge the computed by re-reading the mode.
    mode.value = "system";
  };
  query.addEventListener("change", sync);
}

followSystemScheme();

export function useAppearance() {
  return { mode, isDark, setMode };
}
