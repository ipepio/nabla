/**
 * Chat preferences shared by the whole webview (S2/S3 of
 * `odd/tasks/chat-preferences-and-queue.md`) and the drawer's tool switches
 * (S5 of `odd/tasks/tool-drawer-and-mcp.md`).
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
 * S5 of `tool-drawer-and-mcp.md`: the tool ids a build knows, in the order the
 * settings pane lists them.
 *
 * This list is the SWITCH catalogue, not the tab registry: `browser` has no
 * component yet (F3), so it is offered here as a preference that no tab consumes
 * today. Keeping the two lists apart is what stops a switch from implying a tab
 * that does not exist.
 */
export const TOOL_IDS = ["files", "changes", "terminal", "browser"] as const;
export type ToolPreferenceId = (typeof TOOL_IDS)[number];

/**
 * S5: the tools a fresh install, or a settings file written before this
 * preference existed, offers. Files and Changes are read-only views, so they ship
 * on; Terminal and Browser execute code, so they are opt-in (F2/F3).
 */
export const DEFAULT_ENABLED_TOOLS: readonly string[] = ["files", "changes"];

/**
 * S5: the drawer tools the human enabled. A ref at module scope because the
 * registry-backed tab strip and the settings pane must read one value, and a
 * flip in the dialog has to reach an already-open drawer without a reload.
 */
export const enabledTools = ref<string[]>([...DEFAULT_ENABLED_TOOLS]);

/** S5: whether one tool id is enabled right now. Unknown ids read as off. */
export function isToolEnabled(id: string): boolean {
  return enabledTools.value.includes(id);
}

/**
 * S5: normalizes a stored list so the switch rows stay in catalogue order and a
 * duplicate cannot appear.
 *
 * An id this build does not know is KEPT, after the known ones: the Rust side
 * writes such an id back verbatim (`Settings::enabled_tools`), so dropping it here
 * would let one toggle erase a newer build's preference. It simply matches no
 * registry entry, so it renders nothing.
 */
function normalizeEnabledTools(value: readonly string[] | undefined): string[] {
  if (value === undefined) return [...DEFAULT_ENABLED_TOOLS];
  const known = TOOL_IDS.filter((id) => value.includes(id));
  const unknown = [
    ...new Set(value.filter((id) => !(TOOL_IDS as readonly string[]).includes(id))),
  ];
  return [...known, ...unknown];
}

/**
 * Seeds every preference from a settings payload that was just read (the chat
 * panel on startup, the settings dialog on every open, so a hand edit of the file
 * is picked up). A payload from a build that predates a field carries it as
 * `undefined`; that reads as the documented default and never as "off".
 */
export function applyPreferences(settings: AppSettings | null): void {
  showReasoning.value = settings?.showReasoning ?? true;
  queueMessages.value = settings?.queueMessages ?? true;
  enabledTools.value = normalizeEnabledTools(settings?.enabledTools);
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

/**
 * S5 of `tool-drawer-and-mcp.md`: enables or disables one drawer tool. The live
 * ref changes first — an open drawer drops or gains the tab immediately — and the
 * normalized list is then written through the same read-merge-save path as every
 * other preference.
 *
 * The list is normalized around the toggle, not appended to: the known ids keep
 * catalogue order and any id a newer build wrote is preserved untouched (see
 * `normalizeEnabledTools`).
 */
export async function setToolEnabled(
  id: ToolPreferenceId,
  next: boolean,
): Promise<void> {
  const current = normalizeEnabledTools(enabledTools.value);
  const updated = next
    ? [...current.filter((entry) => entry !== id), id]
    : current.filter((entry) => entry !== id);
  enabledTools.value = normalizeEnabledTools(updated);
  await persist({ enabledTools: enabledTools.value });
}

/** The refs plus the seed, for components that want the whole set at once. */
export function usePreferences() {
  return {
    showReasoning,
    queueMessages,
    enabledTools,
    isToolEnabled,
    applyPreferences,
  };
}
