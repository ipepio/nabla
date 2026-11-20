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
import type { AppSettings, ThinkingLevel } from "@/contracts/types";

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
 * S2 of `odd/tasks/composer-inputs.md`: the levels pi documents for `--thinking`
 * (pi docs/cli.md:71), in pi's own order. Mirrors `THINKING_LEVELS` in
 * `src-tauri/src/settings.rs`.
 *
 * This is the fallback the effort selector offers when the session cannot say
 * which levels its model accepts (no live process yet): pi clamps whatever it is
 * given to the model, so an option the model does not support degrades to the
 * nearest one instead of becoming an error.
 */
export const THINKING_LEVELS: readonly ThinkingLevel[] = [
  "off",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
];

/**
 * S2: the effort a NEW session starts with (the Rust default, pi's documented
 * `defaultThinkingLevel`). An absent field in the file reads as this.
 */
export const DEFAULT_THINKING_LEVEL: ThinkingLevel = "medium";

/**
 * S2: the persisted thinking level, i.e. what the next session starts at. A
 * session that pi already confirmed a level for shows THAT one instead (the
 * selector reads `sessionThinkingLevel` first): pi clamps, so its confirmation
 * wins over this default. Kept at module scope like the other preferences, so a
 * fresh session and the composer cannot disagree about where the agent starts.
 */
export const thinkingLevel = ref<ThinkingLevel>(DEFAULT_THINKING_LEVEL);

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

/**
 * S1 of `odd/tasks/web-search.md`: the agent's `web_search` tool. Default `false`
 * — the query leaves this machine for a third-party server, so the capability is
 * opt-in, exactly like the Rust default for a fresh install and for a settings
 * file written before the preference existed.
 */
export const webSearchEnabled = ref(false);

/**
 * S1: the SearXNG instance that answers the search, as typed. Empty (the default)
 * means "any public instance", which is the best-effort path: public instances
 * rate-limit or refuse the JSON API, so a configured one is the reliable choice.
 * Kept verbatim — Rust normalizes a bare host when it builds the request.
 */
export const webSearchInstance = ref("");

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
  // S2 of `composer-inputs.md`: an absent field reads as the documented default;
  // a word this build does not know is kept verbatim, because that is what the
  // file holds and what the selector must not quietly rewrite.
  const level = settings?.thinkingLevel?.trim() ?? "";
  thinkingLevel.value = level === "" ? DEFAULT_THINKING_LEVEL : level;
  // S1 of `web-search.md`: an absent field reads as the documented default, which
  // is OFF and no instance — never "on, anywhere" by omission.
  webSearchEnabled.value = settings?.webSearchEnabled ?? false;
  webSearchInstance.value = settings?.webSearchInstance ?? "";
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
 * S1 of `web-search.md`: turns the agent's `web_search` tool on or off and
 * persists the choice. Rust re-reads the file on the next tool call, so a flip
 * here reaches the running session without a relaunch.
 */
export async function setWebSearchEnabled(next: boolean): Promise<void> {
  webSearchEnabled.value = next;
  await persist({ webSearchEnabled: next });
}

/**
 * S1: stores the instance the human typed. Trimmed, because a stray space in a
 * url is a typo nobody can see in the field; an empty value is a real choice (any
 * public instance), never "keep the old one".
 */
export async function setWebSearchInstance(next: string): Promise<void> {
  const trimmed = next.trim();
  webSearchInstance.value = trimmed;
  await persist({ webSearchInstance: trimmed });
}

/**
 * S2 of `composer-inputs.md`: changes the effort a NEW session will start with.
 *
 * Only the composer uses this path when there is no session to change — the
 * default is then the whole effect. A session that exists goes through
 * `set_session_thinking_level` instead, which applies the level to that live
 * child and persists the same default in one call (Rust owns that write, so the
 * two can never disagree about the file).
 *
 * The live ref changes first: the selector must not wait for a file write, and a
 * write that never lands leaves the human looking at what they picked, with the
 * file as the next read's truth.
 */
export async function setThinkingLevel(next: ThinkingLevel): Promise<void> {
  thinkingLevel.value = next;
  await persist({ thinkingLevel: next });
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
    thinkingLevel,
    webSearchEnabled,
    webSearchInstance,
    isToolEnabled,
    applyPreferences,
  };
}
