/**
 * The one `settings.json` read the app already paid for at boot (S2 of
 * `odd/tasks/drawer-responsiveness-oauth.md`).
 *
 * `main.ts` reads the file before the first mount to apply the persisted
 * language. `ChatPanel` then read the SAME file again on mount for the theme and
 * the chat preferences, so startup waited for two identical command round-trips
 * of a file nothing had written in between.
 *
 * This module is that hand-off and nothing else: `main.ts` remembers the payload
 * it already holds, and the chat's startup load takes it instead of asking again.
 *
 * TAKING IS ONE-SHOT ON PURPOSE. Only the startup read may reuse the boot payload;
 * after that the cache is empty, so every later read — the settings dialog, the
 * read-merge-save of a theme or model write — asks the command again. That is what
 * keeps a hand-edited file, or a write from another surface, from being answered
 * with a payload read before the app was even mounted.
 *
 * The payload is the raw `AppSettings` the boot read produced, never a partial
 * copy: the chat applies it through the same `applyPreferences`/`setMode` path it
 * uses for a fresh read, so the two sources cannot drift.
 *
 * A boot read that failed remembers nothing: the chat then takes the ordinary
 * read path and reports the failure where it always did.
 */
import type { AppSettings } from "@/contracts/types";

/** The untaken boot read, or `null` when there is none to hand over. */
let pending: AppSettings | null = null;

/**
 * Hands the payload the boot read produced to the next startup consumer. Called
 * once, from `main.ts`, right after a successful read.
 */
export function rememberBootSettings(settings: AppSettings): void {
  pending = settings;
}

/**
 * The boot read, if nobody took it yet. Consumes it: a second call answers `null`,
 * which is how a caller knows it has to read the file itself.
 */
export function takeBootSettings(): AppSettings | null {
  const settings = pending;
  pending = null;
  return settings;
}
