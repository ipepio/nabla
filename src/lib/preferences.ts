/**
 * Chat preferences shared by the whole webview (S2/S3 of
 * `odd/tasks/chat-preferences-and-queue.md`).
 *
 * Same shape as `src/lib/appearance.ts`: the state lives at module scope, so the
 * settings dialog, the chat panel and the session store read one value instead of
 * keeping three copies in sync. "One owner per piece of state" (S4 of
 * `session-lifecycle.md`) is the rule this module exists for.
 *
 * Persistence stays in `settings.json`, as it does for the theme: a setter
 * updates the live ref (the UI must not wait for a file write) and then persists
 * through the existing settings flow — `get_settings` → merge → `save_settings`,
 * which re-reads first so a write cannot clobber a preference another surface
 * changed in the meantime (`save_settings` replaces the file).
 *
 * Both preferences belong to the *chat*, so they are read by non-component code
 * too: `session-store.ts` asks `queueMessages` whether a finished turn should
 * hand the next queued prompt to the session. That is why the refs are exported
 * directly and not only through a `use…()` helper.
 *
 * A failed read or write is swallowed on purpose: a preference is a convenience,
 * and a settings-problem must never break the composer or the transcript. The
 * cached value stays what the UI is already showing.
 */
import { ref } from "vue";
import { getSettings, saveSettings } from "@/contracts/commands";
import type { AppSettings } from "@/contracts/types";

/**
 * S2: the reasoning block is rendered. Default `true` — the Rust default for both
 * a fresh install and a settings file written before this preference existed.
 */
export const showReasoning = ref(true);

/**
 * S3: a prompt sent while the agent is working is queued instead of blocked.
 * Default `true`, like pi.
 */
export const queueMessages = ref(true);

/**
 * Seeds both refs from a settings payload that was just read (the chat panel on
 * startup, the settings dialog on every open, so a hand edit of the file is
 * picked up). A payload from a build that predates the two fields carries them as
 * `undefined`; that reads as the documented default `true`, never as "off".
 */
export function applyPreferences(settings: AppSettings | null): void {
  showReasoning.value = settings?.showReasoning ?? true;
  queueMessages.value = settings?.queueMessages ?? true;
}

/**
 * One write for one preference: re-read the file, merge the new value, save.
 * Never rejects — see the module note on swallowing failures.
 */
async function persist(patch: Partial<AppSettings>): Promise<void> {
  try {
    const current = await getSettings();
    await saveSettings({ ...current, ...patch });
  } catch {
    // A settings write that does not land leaves the live value as the human set
    // it; the next read of the file (opening the dialog, next startup) is the
    // truth. There is no toast layer to report it and a preference is not worth
    // interrupting the chat for.
  }
}

/** S2: shows or hides every reasoning block and persists the choice. */
export async function setShowReasoning(next: boolean): Promise<void> {
  showReasoning.value = next;
  await persist({ showReasoning: next });
}

/** S3: turns the per-session prompt queue on or off and persists the choice. */
export async function setQueueMessages(next: boolean): Promise<void> {
  queueMessages.value = next;
  await persist({ queueMessages: next });
}

/** The refs plus the seed, for components that want the whole set at once. */
export function usePreferences() {
  return { showReasoning, queueMessages, applyPreferences };
}
