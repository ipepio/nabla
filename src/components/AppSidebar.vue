<script setup lang="ts">
// Left drawer (specs S1-S6 of `odd/tasks/sidebar-rework.md`): the Nabla logo and
// the collapse control (the only one in the shell), the "New session" button
// (S2), the Workspaces header with its three actions (S3), the collapsible
// workspace list with each workspace's sessions (S4-S6) and the settings dialog
// in the footer.
//
// It owns the workspace data flow and the session *index* it shows; the
// selection is lifted to App.vue through `select`, which feeds ChatPanel, and the
// active session lives in the shared `src/lib/session-store.ts` (task N1).
// Preferences live in `components/settings/SettingsDialog.vue`, which the footer's
// gear button opens; the drawer only keeps the open flag.
import { computed, defineAsyncComponent, onMounted, onUnmounted, ref, watch } from "vue";
import {
  CirclePlusIcon,
  FolderPlusIcon,
  PanelLeftIcon,
  SearchIcon,
  SettingsIcon,
  SlidersHorizontalIcon,
} from "@lucide/vue";
import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuItem,
  SidebarTrigger,
  useSidebar,
} from "@/components/ui/sidebar";
const SettingsDialog = defineAsyncComponent(
  () => import("@/components/settings/SettingsDialog.vue"),
);
import SessionsList from "@/components/sidebar/SessionsList.vue";
import WorkspaceRow from "@/components/sidebar/WorkspaceRow.vue";
import { useAppearance } from "@/lib/appearance";
import { themedBrand } from "@/lib/brand";
import { t } from "@/lib/i18n";
import { sessionRefresh } from "@/lib/session-refresh";
import {
  activeSession,
  activeSessionTitle,
  busyIds,
  liveIds,
  newSession,
  recordSessionStatus,
  refreshLiveSessions,
  removeSession,
  selectSession,
  sessionGeneration,
} from "@/lib/session-store";
import {
  openSubagentId,
  openSubagentView,
  syncWorkspaceSubagents,
} from "@/lib/subagent-store";
import {
  addWorkspace,
  commandErrorMessage,
  deleteSession,
  listSessions,
  listWorkspaces,
  removeWorkspace,
  renameSession,
  renameWorkspace,
  setWorkspacePinned,
  stopSession,
  touchWorkspace,
} from "../contracts/commands";
import type { SessionSummary, SubagentSession, WorkspaceInfo } from "../contracts/types";

const props = defineProps<{ activeWorkspaceId: number | null }>();
const emit = defineEmits<{ select: [workspace: WorkspaceInfo | null] }>();

const workspaces = ref<WorkspaceInfo[]>([]);
const workspacesLoading = ref(false);
const search = ref("");
/** S3: the magnifier toggles the search field instead of it always taking room. */
const searchOpen = ref(false);
/**
 * S3: the sliders button toggles edit mode. This unit only stores the flag and
 * reflects it on the rows (their actions stay visible); the rename/delete
 * dialogs land in the next unit.
 */
const editMode = ref(false);
// The space whose pin request is in flight, so its button cannot double-fire.
const pinPendingId = ref<number | null>(null);
const adding = ref(false);
/** A `start_session` call is in flight, so the button cannot spawn two pis. */
const creating = ref(false);
const workspaceError = ref<string | null>(null);

/**
 * L4: how often the drawer re-asks which sessions have a live process. The
 * stream already updates the list on every lifecycle event, so this is only a
 * slow safety net for a runtime that never reported one.
 */
const LIVE_POLL_MS = 5000;
let livePollHandle: number | null = null;

/**
 * S5: how often the open session lists re-read their index. A session finishing
 * a turn in another workspace — or one started elsewhere — changes `updated_at`
 * and therefore the order, and nothing in this drawer is told about it, so the
 * list has to ask. `list_sessions` is a SQLite read over an indexed table, so
 * this stays cheap; it is never shorter than the human can perceive.
 */
const SESSION_POLL_MS = 15_000;
let sessionPollHandle: number | null = null;

/** S5: ids of the workspaces whose session list is open. */
const expandedIds = ref<number[]>([]);
/** S5: the sessions of each workspace, keyed by workspace id. */
const sessionsByWorkspace = ref<Record<number, SessionSummary[]>>({});
/**
 * S5: how many `list_sessions` requests are still in flight per workspace. A
 * count, not a flag, so the indicator stays on for the whole burst instead of
 * flickering off between two requests of the same workspace.
 */
const sessionsPending = ref<Record<number, number>>({});
const sessionsErrors = ref<Record<number, string>>({});
/**
 * S5: per-workspace request sequence. Every fetch stamps a new number and a
 * response may only be applied while its number is still the newest one for
 * that workspace, so a slow older `list_sessions` can never overwrite the list
 * a newer request already landed (the stale-response race behind the reorder
 * that never showed up: send touches `updated_at`, the second response wins).
 */
const sessionRequestSequence = new Map<number, number>();
/**
 * A row action (pause / delete) that failed, per workspace. Kept apart from the
 * fetch error so the list still renders: an action must never break it.
 */
const sessionActionErrors = ref<Record<number, string>>({});

// The brand mark is theme-aware: the white-on-dark variant on the dark palette
// and its black-on-light sibling otherwise, so a theme switch switches the logo
// too. `logoFailed` keeps the ∇ fallback while an asset is missing (S7).
const logoFailed = ref(false);
const { isDark } = useAppearance();
// Referenced as a runtime URL (not a static asset import) because Vite fails the
// build for a missing asset, and the file is expected to be missing at first.
const logoSource = computed(() => themedBrand("mark", isDark.value));
// Wordmark only (no mark): the header composes mark + wordmark side by side, so
// it must never show the combined asset (that would duplicate the mark).
const wordmarkFailed = ref(false);
const wordmarkSource = computed(() => themedBrand("wordmark", isDark.value));
// `SidebarTrigger` renders a fixed icon, so the collapsed state needs the raw
// toggle: there the app mark *is* the expand control.
const { toggleSidebar } = useSidebar();

// S1 of `settings-dialog.md`: the drawer's gear button opens the two-pane
// settings dialog, which owns the preference reads and writes. Mounted lazily
// on first open so the settings bundle is never parsed during app startup.
const settingsOpen = ref(false);
const settingsLoaded = ref(false);
function openSettings() {
  settingsLoaded.value = true;
  settingsOpen.value = true;
}

const isMac =
  typeof navigator !== "undefined" &&
  /mac/i.test(navigator.platform || navigator.userAgent);
const newSessionShortcut = isMac ? "⌘ N" : "Ctrl+N";

defineExpose({ openSettings, handleNewSession });
// S6 (next unit): the ⋯ menu's edit item opens this read-only placeholder.
const editOpen = ref(false);
const editingWorkspace = ref<WorkspaceInfo | null>(null);
const editWorkspaceName = ref("");
const savingWorkspaceRename = ref(false);

/**
 * `add_workspace` rejects with a plain Display string. The one rejection worth
 * translating is the store's UNIQUE constraint on `path`: the raw SQLite text
 * ("store sqlite error: UNIQUE constraint failed: workspaces.path") tells a user
 * nothing about what to do next.
 */
function addErrorMessage(cause: unknown): string {
  const message = commandErrorMessage(cause);
  // The SQLite rejection is the only one this path translates; the rest are
  // backend messages rendered as they arrive.
  if (message.includes("UNIQUE constraint failed")) {
    return t("sidebar.duplicatePath");
  }
  return message;
}

/**
 * S3/S4: the search text narrows the loaded list by name or by path. The order
 * stays the server's (pinned first, then most recently used).
 */
const visibleWorkspaces = computed<WorkspaceInfo[]>(() => {
  const query = search.value.trim().toLowerCase();
  if (query === "") return workspaces.value;
  return workspaces.value.filter(
    (workspace) =>
      workspace.name.toLowerCase().includes(query) ||
      workspace.path.toLowerCase().includes(query),
  );
});

async function refreshWorkspaces() {
  workspacesLoading.value = true;
  try {
    workspaces.value = await listWorkspaces();
    workspaceError.value = null;
    // Keep the selection valid and current: drop it if the active row
    // disappeared (otherwise the chat would hold a deleted workspace), and
    // re-emit the refreshed row so pinned/lastUsedAt changes reach App.vue.
    const active = workspaces.value.find((w) => w.id === props.activeWorkspaceId);
    emit("select", active ?? workspaces.value[0] ?? null);
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  } finally {
    workspacesLoading.value = false;
  }
}

/**
 * S8: selecting is also a usage signal. The selection lands immediately, then
 * `touch_workspace` records it and the refresh re-sorts the list by recency.
 */
async function handleSelectWorkspace(workspace: WorkspaceInfo) {
  emit("select", workspace);
  try {
    await touchWorkspace(workspace.id);
    await refreshWorkspaces();
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  }
}

/**
 * S5: a row click selects the workspace (the existing path) and toggles its
 * session list. Clicking the already-selected row only toggles: re-touching it
 * would rewrite recency for an action that changed nothing.
 */
async function handleActivateWorkspace(workspace: WorkspaceInfo) {
  toggleExpanded(workspace.id);
  if (workspace.id !== props.activeWorkspaceId) {
    await handleSelectWorkspace(workspace);
  }
}

function isExpanded(workspaceId: number): boolean {
  return expandedIds.value.includes(workspaceId);
}

function toggleExpanded(workspaceId: number) {
  expandedIds.value = isExpanded(workspaceId)
    ? expandedIds.value.filter((id) => id !== workspaceId)
    : [...expandedIds.value, workspaceId];
}

/** S5: the cached sessions of a workspace, empty until its first fetch lands. */
function sessionsFor(workspaceId: number): SessionSummary[] {
  const cached = sessionsByWorkspace.value[workspaceId] ?? [];
  // Defensive ordering: the query already sorts by activity, but sorting here
  // too means any refresh — poll, send or turn end — lands in the right order
  // even if a response arrives out of band. `updatedAt` is ISO-8601, so the
  // string comparison is the chronological one.
  return [...cached].sort((a, b) => {
    if (a.updatedAt === b.updatedAt) return b.id - a.id;
    return a.updatedAt < b.updatedAt ? 1 : -1;
  });
}

function sessionActionErrorFor(workspaceId: number): string | null {
  return sessionActionErrors.value[workspaceId] ?? null;
}

/** One row action failed: say so under the rows instead of throwing. */
function setSessionActionError(workspaceId: number, message: string | null) {
  const next = { ...sessionActionErrors.value };
  if (message === null) {
    delete next[workspaceId];
  } else {
    next[workspaceId] = message;
  }
  sessionActionErrors.value = next;
}

function sessionsLoadingFor(workspaceId: number): boolean {
  return (sessionsPending.value[workspaceId] ?? 0) > 0;
}

/** One more request is in flight: the indicator stays on until the last lands. */
function markSessionRequestStarted(workspaceId: number) {
  sessionsPending.value = {
    ...sessionsPending.value,
    [workspaceId]: (sessionsPending.value[workspaceId] ?? 0) + 1,
  };
}

function markSessionRequestSettled(workspaceId: number) {
  const remaining = (sessionsPending.value[workspaceId] ?? 1) - 1;
  const next = { ...sessionsPending.value };
  if (remaining > 0) {
    next[workspaceId] = remaining;
  } else {
    delete next[workspaceId];
  }
  sessionsPending.value = next;
}

/** The response's sequence is still the newest one requested for its workspace. */
function isLatestSessionRequest(workspaceId: number, sequence: number): boolean {
  return sessionRequestSequence.get(workspaceId) === sequence;
}

/**
 * S5: whether an incoming index is the one already on screen — same sessions,
 * in the same order, each with the same `updatedAt`. The poll runs every
 * `SESSION_POLL_MS`, so an answer that changes nothing must not touch the
 * reactive state: the rows keep their identity and do not re-render.
 *
 * `updatedAt` is part of the comparison because it is both the sort key and the
 * age label: a turn that completed without reordering the rows still moved it.
 */
function sameSessionList(current: SessionSummary[], next: SessionSummary[]): boolean {
  if (current.length !== next.length) return false;
  return current.every((session, index) => {
    const incoming = next[index];
    return session.id === incoming.id && session.updatedAt === incoming.updatedAt;
  });
}

/**
 * S5: reads one workspace's session index. `list_sessions` is the only source;
 * the cache exists so the list keeps rendering while a refresh is in flight.
 *
 * Every call starts its own request — a refresh is never coalesced away or
 * dropped because another one was running — and the newest one always wins:
 * responses are applied only when they belong to the latest request of that
 * workspace, so the older, slower answer (the one carrying the pre-send order)
 * is discarded instead of overwriting it.
 */
async function loadSessions(workspaceId: number) {
  const sequence = (sessionRequestSequence.get(workspaceId) ?? 0) + 1;
  sessionRequestSequence.set(workspaceId, sequence);
  markSessionRequestStarted(workspaceId);
  try {
    const list = await listSessions(workspaceId);
    // Late answer of a superseded request: the newer one already decided.
    if (!isLatestSessionRequest(workspaceId, sequence)) return;
    // The cache of a collapsed (or removed) workspace was already dropped; a
    // leftover response must not refill it.
    if (!isExpanded(workspaceId)) return;
    // The cache is only replaced once the answer is known, and only when it
    // differs: the list never empties while a fetch is in flight, and a poll
    // that returns the same index leaves the state (and the rendered rows) as
    // they were.
    if (!sameSessionList(sessionsByWorkspace.value[workspaceId] ?? [], list)) {
      sessionsByWorkspace.value = { ...sessionsByWorkspace.value, [workspaceId]: list };
    }
    // H4: the children of the chats just listed, so a delegating chat nests them
    // under its row (S2). This is the lazy trigger the sidebar owns: the visible
    // workspace's sessions are exactly the parents worth asking about, and the
    // shared store pushes the answer into every open row.
    void syncWorkspaceSubagents(
      workspaceId,
      list.map((session) => session.id),
    );
    if (sessionsErrors.value[workspaceId] !== undefined) {
      const settledErrors = { ...sessionsErrors.value };
      delete settledErrors[workspaceId];
      sessionsErrors.value = settledErrors;
    }
  } catch (cause) {
    if (!isLatestSessionRequest(workspaceId, sequence)) return;
    if (!isExpanded(workspaceId)) return;
    sessionsErrors.value = {
      ...sessionsErrors.value,
      [workspaceId]: commandErrorMessage(cause),
    };
  } finally {
    // Only the last settled request turns the indicator off.
    markSessionRequestSettled(workspaceId);
  }
}

/**
 * S5: fetches every expanded workspace. Expanding and re-expanding both come
 * through here, which is why a re-expand refreshes instead of trusting the
 * cache: a session may have started while the row was closed.
 */
function syncSessions(ids: number[]) {
  for (const id of ids) void loadSessions(id);
  // Collapsed rows drop their cache so a later expand cannot show a stale list.
  const keep = new Set(ids);
  sessionsByWorkspace.value = Object.fromEntries(
    Object.entries(sessionsByWorkspace.value).filter(([key]) => keep.has(Number(key))),
  );
}

watch(expandedIds, syncSessions);

/**
 * S5: keeps every *open* session list current, so the drawer reorders on its
 * own while it is visible. It goes through `loadSessions`, which owns the
 * per-workspace sequence guard and the pending counter, so a poll answer is
 * never newer than the send-triggered refresh it races with and never runs on
 * top of a fetch already in flight.
 *
 * A hidden document is paused: the human cannot see the list, and coming back
 * refreshes once immediately instead of replaying the missed ticks.
 */
function pollExpandedSessions() {
  if (document.visibilityState !== "visible") return;
  for (const workspaceId of expandedIds.value) {
    // Nothing is open for this workspace when a fetch is already running; a
    // second request would only queue a redundant answer behind the first.
    if (sessionsLoadingFor(workspaceId)) continue;
    void loadSessions(workspaceId);
  }
}

/** Coming back from a hidden document refreshes once, right away. */
function handleVisibilityChange() {
  if (document.visibilityState === "visible") pollExpandedSessions();
}

// N1: a new session (from the button or from the composer's first send) changes
// what an open list must show, exactly like a finished turn does. The drawer used
// to answer every index change by dropping the whole cache and refetching every
// open workspace; it now asks the planner which open list actually changed (S2 of
// `drawer-responsiveness-oauth.md`).
//
// What is deliberately unchanged: the refresh still goes through `loadSessions`,
// which owns the per-workspace sequence guard, the pending counter and the
// replace-only-when-it-differs rule — so the rows on screen are kept while the
// re-read is in flight and replaced only when a different index lands. Nothing
// clears `sessionsByWorkspace` here any more: a turn landing in one workspace must
// not blank the rows of the others.
function applySessionRefresh() {
  const plan = sessionRefresh.takePlan(expandedIds.value);
  for (const workspaceId of plan.workspaces) void loadSessions(workspaceId);
}

// Activity (a prompt sent, a turn finished, a session created) arrives through
// the planner's signal; the index-shape counter is watched too, so a creation or a
// cleared selection still reaches the open lists.
watch(sessionRefresh.signal, applySessionRefresh);
watch(sessionGeneration, applySessionRefresh);

/** S2: creates a session for the selected workspace — the one creation path. */
async function handleNewSession() {
  const workspace = workspaces.value.find((w) => w.id === props.activeWorkspaceId);
  if (workspace === undefined || creating.value) return;
  creating.value = true;
  workspaceError.value = null;
  try {
    // Same usage signal the selection path records; the list itself is not
    // re-sorted under the pointer here, only on the next select or add.
    await touchWorkspace(workspace.id);
    await newSession(workspace.id);
    // S5: open the row so the session it just created is visible.
    if (!isExpanded(workspace.id)) {
      expandedIds.value = [...expandedIds.value, workspace.id];
    }
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  } finally {
    creating.value = false;
  }
}

/**
 * L4: selecting a session in the list makes it the active session and focuses
 * its workspace, so the chat is never showing one workspace while its session
 * belongs to another. The store does the rest — opening the process on demand
 * when it is not live (S3), reusing it when it is (S5) and seeding the
 * transcript from pi's file. Any session that was running before keeps running
 * (S4): this path never stops anything.
 */
async function handleOpenSession(
  workspace: WorkspaceInfo,
  session: SessionSummary,
) {
  if (workspace.id !== props.activeWorkspaceId) {
    await handleSelectWorkspace(workspace);
  }
  await selectSession(session);
}

/** S3: pin/unpin one row; the refreshed server list is the source of truth. */
async function handleTogglePin(workspace: WorkspaceInfo) {
  if (pinPendingId.value !== null) return;
  pinPendingId.value = workspace.id;
  workspaceError.value = null;
  try {
    await setWorkspacePinned(workspace.id, !workspace.pinned);
    await refreshWorkspaces();
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  } finally {
    pinPendingId.value = null;
  }
}

/**
 * Per-session "Pause": the existing `stop_session`. The row, its transcript and
 * its index entry stay; only the pi process goes, so re-selecting the session
 * opens it again on the same file.
 */
async function handlePauseSession(
  workspace: WorkspaceInfo,
  session: SessionSummary,
) {
  setSessionActionError(workspace.id, null);
  try {
    await stopSession(workspace.id, session.id);
    recordSessionStatus(String(session.id), "stopped", null);
    await refreshLiveSessions();
  } catch (cause) {
    setSessionActionError(workspace.id, commandErrorMessage(cause));
  }
}

/**
 * Per-session "Delete", after the list's own confirmation: Rust stops any live
 * process, archives the transcript and drops the index row; the store forgets
 * the session and the list is re-read from the index. On failure the message
 * lands under the rows — the list itself never breaks.
 */
async function handleDeleteSession(
  workspace: WorkspaceInfo,
  session: SessionSummary,
) {
  setSessionActionError(workspace.id, null);
  try {
    await deleteSession(session.id);
    removeSession(session.id);
    await loadSessions(workspace.id);
  } catch (cause) {
    setSessionActionError(workspace.id, commandErrorMessage(cause));
  }
}

async function handleRenameSession(
  workspace: WorkspaceInfo,
  payload: { session: SessionSummary; title: string },
) {
  setSessionActionError(workspace.id, null);
  const trimmed = payload.title.trim();
  if (trimmed === "") return;
  try {
    await renameSession(payload.session.id, trimmed);
    if (activeSession.value?.sessionId === String(payload.session.id)) {
      activeSessionTitle.value = trimmed;
    }
    await loadSessions(workspace.id);
  } catch (cause) {
    setSessionActionError(workspace.id, commandErrorMessage(cause));
  }
}

/**
 * H4 of `odd/tasks/subagent-helpers.md`: a child row opens the read-only view of
 * that child — never a conversation, and never through `selectSession` for the
 * child itself (a child is not a chat Nabla runs; selecting it would spawn a pi for
 * its file).
 *
 * The parent chat is focused only when its pi is already live, and that is the
 * reason for the guard below: selecting a stopped session spawns one, and a click on
 * a child must not be a spawn path. When the parent is not live the view still opens
 * and the back control returns to whatever chat is on screen.
 */
async function handleOpenSubagent(
  workspace: WorkspaceInfo,
  opened: { child: SubagentSession; parent: SessionSummary },
) {
  const parent = opened.parent;
  const child = opened.child;
  if (workspace.id !== props.activeWorkspaceId) await handleSelectWorkspace(workspace);
  if (
    activeSession.value?.sessionId !== String(parent.id) &&
    liveIds.value.includes(String(parent.id))
  ) {
    await selectSession(parent);
  }
  openSubagentView(child, String(parent.id), workspace.id);
}

/**
 * S3: the header's folder button. The native folder picker is the whole add
 * flow; cancelling the dialog is not an error, so it silently does nothing. The
 * name is never sent: the store derives it from the path (S6's default).
 */
async function handleBrowseFolder() {
  if (adding.value) return;
  workspaceError.value = null;
  let selected: string | string[] | null;
  try {
    selected = await openFolderDialog({
      directory: true,
      multiple: false,
      title: t("sidebar.chooseFolder"),
    });
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
    return;
  }
  const path = Array.isArray(selected) ? selected[0] : selected;
  if (!path) return;
  await addWorkspaceAndSelect(path, null);
}

/** Create the row, refresh, and select it — adding a workspace is an intent. */
async function addWorkspaceAndSelect(path: string, name: string | null) {
  adding.value = true;
  workspaceError.value = null;
  try {
    const created = await addWorkspace(path, name);
    await refreshWorkspaces();
    const added = workspaces.value.find((w) => w.id === created.id) ?? created;
    emit("select", added);
  } catch (cause) {
    workspaceError.value = addErrorMessage(cause);
  } finally {
    adding.value = false;
  }
}

async function handleRemoveWorkspace(workspace: WorkspaceInfo) {
  workspaceError.value = null;
  try {
    await removeWorkspace(workspace.id);
    // The row is gone: drop its cached sessions and its expansion as well.
    expandedIds.value = expandedIds.value.filter((id) => id !== workspace.id);
    delete sessionsByWorkspace.value[workspace.id];
    delete sessionsErrors.value[workspace.id];
    // Its revision cannot describe anything any more, so a re-created workspace
    // never inherits the activity of the row that was removed.
    sessionRefresh.forget(workspace.id);
    // H4: its children go with it. An empty list prunes every parent the store was
    // holding for this workspace, so nothing keeps polling children nobody lists.
    void syncWorkspaceSubagents(workspace.id, []);
    // `refreshWorkspaces` replaces the selection when the active row is gone.
    await refreshWorkspaces();
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  }
}

/**
 * S6: the ⋯ menu's edit item. Rename/alias and the delete confirmation are the
 * next unit's work; until then this opens a read-only dialog that shows exactly
 * what the store holds, so nothing is silently pretended.
 */
function openEditDialog(workspace: WorkspaceInfo) {
  editingWorkspace.value = workspace;
  editWorkspaceName.value = workspace.name;
  editOpen.value = true;
}

async function handleSaveWorkspaceRename() {
  const ws = editingWorkspace.value;
  const next = editWorkspaceName.value.trim();
  if (!ws || next === "" || savingWorkspaceRename.value) return;
  savingWorkspaceRename.value = true;
  workspaceError.value = null;
  try {
    await renameWorkspace(ws.id, next);
    await refreshWorkspaces();
    editOpen.value = false;
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  } finally {
    savingWorkspaceRename.value = false;
  }
}

onMounted(() => {
  void refreshWorkspaces();
  // The persisted theme is applied by the chat panel at startup (its settings
  // load calls `setMode`); this drawer only opens the settings dialog.
  // L4: which sessions have a live process. Events keep this current (the store
  // updates it on every lifecycle frame); the interval only covers the case the
  // drawer needs after a process started or died while it was not listening.
  void refreshLiveSessions();
  livePollHandle = window.setInterval(() => {
    void refreshLiveSessions();
  }, LIVE_POLL_MS);
  // S5: the open session lists follow their index on their own (a turn landing
  // in another workspace reorders them), paused while the window is hidden.
  document.addEventListener("visibilitychange", handleVisibilityChange);
  sessionPollHandle = window.setInterval(pollExpandedSessions, SESSION_POLL_MS);
});

onUnmounted(() => {
  if (livePollHandle !== null) {
    window.clearInterval(livePollHandle);
    livePollHandle = null;
  }
  if (sessionPollHandle !== null) {
    window.clearInterval(sessionPollHandle);
    sessionPollHandle = null;
  }
  document.removeEventListener("visibilitychange", handleVisibilityChange);
});
</script>

<template>
  <Sidebar collapsible="icon">
    <SidebarHeader class="gap-3 px-3 py-3">
      <div class="flex items-center gap-2">
        <!-- Expanded header, one row: [mark] [wordmark] [collapse control].
             The drawer is still the only place with a collapse control (S8). -->
        <div
          class="flex min-w-0 flex-1 items-center gap-2 group-data-[collapsible=icon]:hidden"
        >
          <!-- Brand mark (S7), no text: the wordmark asset sits next to it. -->
          <img
            v-if="!logoFailed"
            :src="logoSource"
            alt=""
            aria-hidden="true"
            class="size-6 shrink-0 rounded-md object-contain"
            @error="logoFailed = true"
          />
          <!-- Nabla glyph fallback: the del operator, the app's namesake. -->
          <span
            v-else
            aria-hidden="true"
            class="text-lg leading-none font-semibold text-primary select-none"
            >∇</span
          >

          <!-- Wordmark only, centered in the gap between mark and trigger, so the
               row never repeats the mark twice. -->
          <span class="flex min-w-0 flex-1 items-center justify-center">
            <img
              v-if="!wordmarkFailed"
              :src="wordmarkSource"
              alt="Nabla"
              class="h-3 w-auto shrink-0 object-contain"
              @error="wordmarkFailed = true"
            />
            <span v-else class="truncate text-xs font-semibold">Nabla</span>
          </span>

          <SidebarTrigger class="shrink-0" />
        </div>

        <!-- Collapsed: the app mark *is* the expand control, so the rail gains
             no extra chrome beyond the logo. -->
        <Button
          variant="ghost"
          size="icon-sm"
          class="group/expand mx-auto hidden group-data-[collapsible=icon]:flex"
          :aria-label="t('sidebar.expand')"
          @click="toggleSidebar"
        >
          <!-- Brand-first at rest; the expand affordance appears on hover and on
               keyboard focus, so the control stays discoverable without the rail
               losing its logo. -->
          <img
            v-if="!logoFailed"
            :src="logoSource"
            alt="Nabla"
            class="size-6 rounded-md object-contain group-hover/expand:hidden group-focus-visible/expand:hidden"
            @error="logoFailed = true"
          />
          <span
            v-else
            aria-hidden="true"
            class="text-lg leading-none font-semibold group-hover/expand:hidden group-focus-visible/expand:hidden"
            >∇</span
          >
          <PanelLeftIcon
            class="hidden group-hover/expand:block group-focus-visible/expand:block"
          />
        </Button>
      </div>

      <!-- S2: New session, full width, just under the header. It creates the
           session for the selected workspace through the shared store, so the
           chat follows it; collapsed, the label gives way to the icon. -->
      <Button
        variant="secondary"
        class="h-9 w-full justify-start gap-2.5 rounded-lg border border-border bg-secondary px-3 text-xs font-semibold text-secondary-foreground transition-colors hover:border-primary/40 hover:bg-accent group-data-[collapsible=icon]:size-8 group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:self-center group-data-[collapsible=icon]:p-0"
        :disabled="activeWorkspaceId === null || creating"
        :aria-label="t('sidebar.newSession')"
        @click="handleNewSession"
      >
        <CirclePlusIcon class="size-4 shrink-0 text-primary" aria-hidden="true" />
        <span class="group-data-[collapsible=icon]:hidden">
          {{ t("sidebar.newSession") }}
        </span>
        <kbd
          class="ms-auto rounded border border-border bg-card px-1.5 py-0.5 font-mono text-[10px] font-medium text-muted-foreground group-data-[collapsible=icon]:hidden"
        >
          {{ newSessionShortcut }}
        </kbd>
      </Button>
    </SidebarHeader>

    <!-- S1: no separator between the header and the content. -->
    <SidebarContent class="px-2 py-2">
      <!-- S3: section title on the left, the three actions on the right. -->
      <div
        class="flex items-center justify-between gap-1 px-1.5 pt-2 pb-1.5 group-data-[collapsible=icon]:hidden"
      >
        <span class="text-[10px] font-bold tracking-widest text-muted-foreground uppercase">
          {{ t("sidebar.workspaces") }}
        </span>
        <div class="flex items-center gap-0.5">
          <!-- Search: toggles the field below instead of it always taking room. -->
          <Button
            variant="ghost"
            size="icon-xs"
            class="text-muted-foreground"
            :aria-label="t('sidebar.search')"
            :aria-pressed="searchOpen"
            :aria-expanded="searchOpen"
            @click="searchOpen = !searchOpen"
          >
            <SearchIcon />
          </Button>
          <!-- Edit mode: the flag this unit stores; rows keep their actions
               visible while it is on (the dialogs come next). -->
          <Button
            variant="ghost"
            size="icon-xs"
            :class="editMode ? 'text-foreground' : 'text-muted-foreground'"
            :aria-label="t('sidebar.editMode')"
            :aria-pressed="editMode"
            @click="editMode = !editMode"
          >
            <SlidersHorizontalIcon />
          </Button>
          <!-- Add folder: the native picker, the whole add flow (S3). -->
          <Button
            variant="ghost"
            size="icon-xs"
            class="text-muted-foreground"
            :aria-label="t('sidebar.addWorkspace')"
            :disabled="adding"
            @click="handleBrowseFolder"
          >
            <FolderPlusIcon />
          </Button>
        </div>
      </div>

      <div v-if="searchOpen" class="px-2 pb-1 group-data-[collapsible=icon]:hidden">
        <div class="relative">
          <SearchIcon
            aria-hidden="true"
            class="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground"
          />
          <Input
            v-model="search"
            :aria-label="t('sidebar.search')"
            :placeholder="t('sidebar.searchPlaceholder')"
            class="h-8 pl-7 text-xs"
          />
        </div>
      </div>

      <!-- S5: the collapsible list. The divider that used to sit here was
           removed: the section header already separates the groups. -->
      <div class="flex min-h-0 flex-col gap-1 px-0.5 pb-1">
        <p
          v-if="workspaces.length === 0 && !workspacesLoading"
          class="px-2 py-1 text-xs text-muted-foreground group-data-[collapsible=icon]:hidden"
        >
          {{ t("sidebar.empty") }}
        </p>
        <!-- Search came back empty: distinct from "no workspaces". -->
        <p
          v-else-if="!workspacesLoading && visibleWorkspaces.length === 0"
          class="px-2 py-1 text-xs text-muted-foreground group-data-[collapsible=icon]:hidden"
        >
          {{ t("sidebar.emptySearch") }}
        </p>

        <SidebarMenu class="gap-1.5">
          <SidebarMenuItem
            v-for="workspace in visibleWorkspaces"
            :key="workspace.id"
            class="overflow-hidden rounded-lg border border-sidebar-border bg-card/60 transition-colors group-data-[collapsible=icon]:border-transparent group-data-[collapsible=icon]:bg-transparent"
            :class="workspace.id === activeWorkspaceId ? 'border-border bg-card' : ''"
          >
            <WorkspaceRow
              :workspace="workspace"
              :active="workspace.id === activeWorkspaceId"
              :expanded="isExpanded(workspace.id)"
              :edit-mode="editMode"
              :pin-pending="pinPendingId === workspace.id"
              @activate="handleActivateWorkspace(workspace)"
              @toggle-pin="handleTogglePin(workspace)"
              @edit="openEditDialog(workspace)"
              @remove="handleRemoveWorkspace(workspace)"
            />
            <!-- S5: its sessions, lazily fetched and cached per workspace. -->
            <SessionsList
              v-if="isExpanded(workspace.id)"
              :sessions="sessionsFor(workspace.id)"
              :loading="sessionsLoadingFor(workspace.id)"
              :error="sessionsErrors[workspace.id] ?? null"
              :active-session-id="activeSession?.sessionId ?? null"
              :live-session-ids="liveIds"
              :busy-session-ids="busyIds"
              :open-subagent-id="openSubagentId"
              :action-error="sessionActionErrorFor(workspace.id)"
              @select="handleOpenSession(workspace, $event)"
              @pause="handlePauseSession(workspace, $event)"
              @rename="handleRenameSession(workspace, $event)"
              @delete="handleDeleteSession(workspace, $event)"
              @open-subagent="handleOpenSubagent(workspace, $event)"
            />
          </SidebarMenuItem>
        </SidebarMenu>
      </div>

      <Alert v-if="workspaceError" variant="destructive" class="mx-2 mb-2">
        <AlertDescription>{{ workspaceError }}</AlertDescription>
      </Alert>
    </SidebarContent>

    <SidebarFooter class="px-2.5 py-2.5">
      <!-- S1 of `settings-dialog.md`: the gear opens the two-pane settings
           dialog; the dialog owns every preference read and write. -->
      <Button
        variant="ghost"
        class="h-8 w-full justify-start gap-2 rounded-lg text-xs text-muted-foreground hover:bg-secondary hover:text-foreground"
        :aria-label="t('sidebar.settings')"
        @click="openSettings"
      >
        <SettingsIcon />
        <span class="group-data-[collapsible=icon]:hidden">
          {{ t("sidebar.settings") }}
        </span>
      </Button>
      <SettingsDialog v-if="settingsLoaded" v-model:open="settingsOpen" />

      <!-- S6: read-only placeholder behind the row menu's "Edit workspace".
           Alias editing and the delete confirmation are the next unit. -->
      <Dialog v-model:open="editOpen">
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{{ t("workspace.edit") }}</DialogTitle>
            <DialogDescription class="break-all font-mono text-xs">
              {{ editingWorkspace?.path }}
            </DialogDescription>
          </DialogHeader>

          <form class="grid gap-3" @submit.prevent="handleSaveWorkspaceRename">
            <div class="grid gap-1.5">
              <label class="text-xs font-medium" for="workspace-rename-input">
                {{ t("common.name") }}
              </label>
              <Input
                id="workspace-rename-input"
                v-model="editWorkspaceName"
                type="text"
                autocomplete="off"
                class="h-9 text-sm"
              />
            </div>

            <DialogFooter>
              <DialogClose as-child>
                <Button type="button" variant="outline">
                  {{ t("common.cancel") }}
                </Button>
              </DialogClose>
              <Button
                type="submit"
                :disabled="!editWorkspaceName.trim() || savingWorkspaceRename"
              >
                {{ t("workspace.rename.save") }}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </SidebarFooter>
  </Sidebar>
</template>
