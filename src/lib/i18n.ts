// Typed, dependency-free i18n (spec S2 of `odd/tasks/i18n-and-settings-polish.md`).
//
// The catalogue is flat and dotted (`settings.general.appearance`), which keeps
// lookups a plain property read and keeps `<key>` stable in reviews. `en.json` is
// the source of truth for the key set: `MessageKey` is derived from it, so a
// component asking for a key that does not exist fails `vue-tsc`, and `es.json`
// is annotated with the same record type so a missing translation fails the same
// way (the parity guard below also rejects an extra key).
//
// Interpolation covers `{name}` only. Pluralization is deliberately out of scope
// until a string actually needs it (L3 of the task doc): no dependency, no
// message-format layer.
import { ref, type Ref } from "vue";
import en from "@/locales/en.json";
import es from "@/locales/es.json";

/** The locales the selector offers, in the order it shows them. */
export const SUPPORTED_LOCALES = ["en", "es"] as const;
export type Locale = (typeof SUPPORTED_LOCALES)[number];

/** Every message key is a key of `en.json`: an unknown key is a build error. */
export type MessageKey = keyof typeof en;
type MessageTable = Record<MessageKey, string>;

/**
 * Compile-time parity guard: `es.json` must carry exactly the keys of `en.json`.
 * An extra Spanish key, or a missing one, widens one side of the comparison and
 * turns this type into `never`, so the assignment below stops compiling.
 */
type ExactKeySet<A extends string, B extends string> = [A] extends [B]
  ? [B] extends [A]
    ? true
    : never
  : never;
export const LOCALE_KEYS_IN_SYNC: ExactKeySet<MessageKey, keyof typeof es> = true;

/** One table per locale; the annotation is the missing-translation guard. */
const messages: Record<Locale, MessageTable> = { en, es };

/**
 * Module state, as the i18n contract fixes it: every `t()` call reads this ref,
 * so a component that renders `t(...)` re-renders when the locale changes.
 */
export const locale = ref<Locale>("en");

function isLocale(value: string): value is Locale {
  return (SUPPORTED_LOCALES as readonly string[]).includes(value);
}

/**
 * Switches the interface language. Anything that is not a supported locale falls
 * back to English: `settings.json` is human-editable, and a typo there must never
 * leave the UI with no strings at all.
 */
export function setLocale(next: Locale | string): void {
  locale.value = isLocale(next) ? next : "en";
  // Screen readers and hyphenation follow the document language, and the webview
  // launches with the one in index.html.
  if (typeof document !== "undefined") {
    document.documentElement.lang = locale.value;
  }
}

/**
 * The message for `key` in the current locale, with `{var}` placeholders
 * replaced by `vars`. An unknown placeholder is left untouched rather than
 * rendered as "undefined".
 */
export function t(
  key: MessageKey,
  vars?: Record<string, string | number>,
): string {
  const text = messages[locale.value][key];
  if (vars === undefined) return text;
  return text.replace(/\{(\w+)\}/g, (placeholder, name: string) =>
    Object.prototype.hasOwnProperty.call(vars, name)
      ? String(vars[name])
      : placeholder,
  );
}

/** The composable form for components that also switch the locale. */
export function useI18n(): {
  locale: Ref<Locale>;
  setLocale: typeof setLocale;
  t: typeof t;
} {
  return { locale, setLocale, t };
}
