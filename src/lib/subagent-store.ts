// The children a chat delegated to, and the read-only view of one of them
// (H4 of `odd/tasks/subagent-helpers.md`, specs S1/S2/S3).
//
// Two rules shape this module, and both are structural rather than advisory:
//
// 1. **A child is read, never driven.** Everything here goes through
//    `subagentSessions` (the children of a chat, status merged live from the
//    extension's manifest) and `sessionTranscript` (the child's own pi JSONL).
//    This module deliberately does not import `openSession`, `startSession` or
//    `sendPrompt`: opening a child can therefore never spawn a pi for it, and the
//    command path stays in the parent chat (S3). `src/lib/session-store.ts` is not
//    involved either — no buffer, no live subscription, no queue — because a child
//    has no process of its own inside Nabla and must not look like a chat the app
//    is running.
//
// 2. **Nothing polls unless something is running.** One interval, started the
//    moment a known child reports `running` (or `queued`, which is a child about
//    to run) and stopped as soon as none does. It re-asks the parents that have a
//    running child and, while the child on screen is running, re-reads its
//    transcript so the view follows the file as it grows (S3: "siguiendo el JSONL
//    mientras crece").
//
// The link between a `run_agent` call and the child it started is the child's own
// session file: the extension answers the call with it and the index row carries
// it, so `resolveSubagentChild` matches on `sessionPath` first and only falls back
// to the agent name.
import { computed, reactive, ref } from "vue";
import {
  commandErrorMessage,
  sessionTranscript,
  subagentSessions,
} from "../contracts/commands";
import type {
  SubagentSession,
  SubagentStatus,
  TranscriptMessage,
} from "../contracts/types";
import { t } from "./i18n";
import type { ChatMessage } from "./session-store";

/** Everything known about one chat's children, keyed by the parent's session id. */
interface ParentChildren {
  /** The workspace the parent belongs to: every query takes the pair. */
  workspaceId: number;
  children: SubagentSession[];
  /** The first read has landed. An empty list is a real answer, not "unknown". */
  loaded: boolean;
  /** The last read failed; the state is shown, not thrown. */
  error: string | null;
}

/**
 * Children per parent chat, shared by the sidebar (which nests them) and the chat
 * panel (whose helper cards point at them), exactly like `session-store` shares
 * the chats themselves: one instance per app, every importer reading the same
 * objects.
 */
const parents = reactive(new Map<string, ParentChildren>());

/**
 * Reads in flight, per parent. `subagent_sessions` is a directory read of the
 * extension's manifests on the Rust side, and both the sidebar poll and a card's
 * mount can ask for the same parent in the same tick — the second caller joins
 * the first instead of asking twice.
 */
const pending = new Map<string, Promise<void>>();

/** How often a running child's state (and the open child's file) is re-read. */
const POLL_MS = 2000;

/** The single interval of this module, or `null` when nothing is running. */
let pollHandle: number | null = null;

// ── live-view state (H4.2) ───────────────────────────────────────────────────

/**
 * The child the chat panel is showing, with the chat that delegated to it.
 *
 * The panel is not navigating: this is a *state* of the same column, which is why
 * going back is a plain reset and nothing is torn down (the parent chat keeps its
 * buffer, its process and its scroll behaviour untouched).
 */
export interface OpenSubagent {
  /** The child being shown, as the parent's list last described it. */
  child: SubagentSession;
  /** The chat that delegated to it; the back control returns there. */
  parentSessionId: string;
  /** The workspace that owns both, so a refresh can address the parent. */
  workspaceId: number;
}

/** The child on screen, or `null` when the panel shows a conversation. */
export const openSubagent = ref<OpenSubagent | null>(null);

/** The child's transcript, already in the shape the turn-group renderer takes. */
export const subagentTranscript = ref<ChatMessage[]>([]);

/** The first read of the child's file is in flight and there is nothing painted. */
export const subagentTranscriptLoading = ref(false);

/** Why the child's transcript could not be read, or `null`. */
export const subagentTranscriptError = ref<string | null>(null);

/** Id of the child on screen; the sidebar marks its row with it. */
export const openSubagentId = computed<number | null>(
  () => openSubagent.value?.child.sessionId ?? null,
);

/**
 * The status of the child on screen, taken from its parent's list — the manifest
 * read on the Rust side — and only from the snapshot the view was opened with when
 * the list knows nothing (yet).
 */
export const openSubagentStatus = computed<SubagentStatus>(() => {
  const open = openSubagent.value;
  if (open === null) return "unknown";
  return subagentStatus(open.parentSessionId, open.child.sessionId, open.child.status);
});

// ── reading the children of a chat ───────────────────────────────────────────

/** A child is (or is about to be) working, so its state is worth polling. */
export function isSubagentActive(status: SubagentStatus): boolean {
  return status === "running" || status === "queued";
}

/** The children of one chat, oldest first as Rust answers them. */
export function subagentChildrenOf(
  parentSessionId: string | number | null,
): SubagentSession[] {
  if (parentSessionId === null) return [];
  return parents.get(String(parentSessionId))?.children ?? [];
}

/** Why the last read of this chat's children failed, or `null`. */
export function subagentChildrenError(
  parentSessionId: string | number | null,
): string | null {
  if (parentSessionId === null) return null;
  return parents.get(String(parentSessionId))?.error ?? null;
}

/**
 * The status of one child as its parent's list reports it — the live manifest
 * word — falling back to `fallback` when the list does not know the child (a view
 * opened before the parent was ever read).
 */
export function subagentStatus(
  parentSessionId: string | null,
  sessionId: number,
  fallback: SubagentStatus = "unknown",
): SubagentStatus {
  const known = subagentChildrenOf(parentSessionId).find(
    (child) => child.sessionId === sessionId,
  );
  return known?.status ?? fallback;
}

/** The word a status is rendered with; the manifest's own vocabulary (S3). */
export function subagentStatusLabel(status: SubagentStatus): string {
  switch (status) {
    case "queued":
      return t("chat.subagent.status.queued");
    case "running":
      return t("chat.subagent.status.running");
    case "done":
      return t("chat.subagent.status.done");
    case "failed":
      return t("chat.subagent.status.failed");
    case "unknown":
      return t("chat.subagent.status.unknown");
  }
}

/**
 * The dot of one status, drawn from the semantic tokens (ADR 0009: no raw colors
 * in components). Running pulses — it is the one state that moves on its own —
 * done stays quiet, failed wears the destructive token, and "we cannot tell" is an
 * outlined dot rather than a colour that would claim a state.
 */
export function subagentStatusDot(status: SubagentStatus): string {
  switch (status) {
    case "queued":
      return "bg-muted-foreground/50";
    case "running":
      return "bg-primary nabla-status-pulse";
    case "done":
      return "bg-tertiary";
    case "failed":
      return "bg-destructive";
    case "unknown":
      return "border border-muted-foreground/60";
  }
}

/**
 * The extension's own word, narrowed to the contract. A word this build does not
 * know reads as `unknown` — never as `done` (a run wrongly reported as finished is
 * worse than one shown without a state) and never as `running` (which would look
 * live forever). Mirrors `manifest_status` in `src-tauri/src/commands.rs`.
 */
export function subagentStatusWord(raw: string | null): SubagentStatus {
  switch ((raw ?? "").trim()) {
    case "queued":
      return "queued";
    case "running":
      return "running";
    case "done":
      return "done";
    case "failed":
      return "failed";
    default:
      return "unknown";
  }
}

/**
 * The child a `run_agent` call started, matched against its parent's children.
 *
 * `sessionPath` decides: the extension answers the call with the child's own pi
 * session file and Rust indexes that same file, so a match is the child itself and
 * not a guess. Only when the file is not known (a call whose result never arrived,
 * or one that failed before creating a session) does the agent name answer, and
 * then the newest child of that agent wins — the run the call most plausibly
 * started. `null` means the card cannot point at a child yet.
 */
export function resolveSubagentChild(
  parentSessionId: string | null,
  link: { sessionPath: string | null; agent: string | null },
): SubagentSession | null {
  const children = subagentChildrenOf(parentSessionId);
  if (children.length === 0) return null;
  const path = link.sessionPath;
  if (path !== null) {
    const byPath = children.find((child) => child.sessionPath === path);
    if (byPath !== undefined) return byPath;
  }
  const agent = link.agent?.trim().toLowerCase();
  if (agent === undefined || agent === "") return null;
  for (let index = children.length - 1; index >= 0; index -= 1) {
    const child = children[index];
    if (child !== undefined && child.agent.trim().toLowerCase() === agent) return child;
  }
  return null;
}

// ── loading ──────────────────────────────────────────────────────────────────

function ensureParentEntry(workspaceId: number, parentSessionId: string): ParentChildren {
  const existing = parents.get(parentSessionId);
  if (existing !== undefined) {
    if (existing.workspaceId !== workspaceId) existing.workspaceId = workspaceId;
    return existing;
  }
  // `reactive` keeps the identity stable, exactly as `ensureBuffer` does for a
  // session: the map stores the raw target and hands back this same proxy.
  const created = reactive<ParentChildren>({
    workspaceId,
    children: [],
    loaded: false,
    error: null,
  });
  parents.set(parentSessionId, created);
  return created;
}

/** Whether two answers describe the same children, in the same state. */
function sameChildren(current: SubagentSession[], next: SubagentSession[]): boolean {
  if (current.length !== next.length) return false;
  return current.every((child, index) => {
    const incoming = next[index];
    return (
      child.sessionId === incoming.sessionId &&
      child.status === incoming.status &&
      child.agent === incoming.agent &&
      child.name === incoming.name &&
      child.endedAt === incoming.endedAt
    );
  });
}

/**
 * Reads one chat's children. The cache is replaced only when the answer differs,
 * so a poll that changes nothing does not re-render a single row; a status that
 * changed does, immediately.
 */
async function loadOne(
  workspaceId: number,
  parentSessionId: string,
  force: boolean,
): Promise<void> {
  const entry = ensureParentEntry(workspaceId, parentSessionId);
  if (!force && entry.loaded) return;
  const running = pending.get(parentSessionId);
  if (running !== undefined) {
    await running;
    return;
  }
  const request = (async () => {
    try {
      const children = await subagentSessions(workspaceId, Number(parentSessionId));
      const current = parents.get(parentSessionId);
      // A workspace change or a workspace removal in the meantime: this answer
      // describes children nobody is looking at any more.
      if (current === undefined || current.workspaceId !== workspaceId) return;
      if (!sameChildren(current.children, children)) current.children = children;
      current.loaded = true;
      current.error = null;
    } catch (cause) {
      const current = parents.get(parentSessionId);
      if (current === undefined || current.workspaceId !== workspaceId) return;
      // Not fatal and not sticky: `loaded` stays down so the next read tries
      // again, and the message is rendered where children would be.
      current.error = commandErrorMessage(cause);
    }
  })();
  pending.set(parentSessionId, request);
  try {
    await request;
  } finally {
    pending.delete(parentSessionId);
    // Whatever landed may have started or ended a run, so the poll follows it.
    syncPolling();
  }
}

/**
 * Reads the children of the given chats, reusing what is already known unless
 * `force` says the answer must be fresh (the sidebar's list load and the poll both
 * do: a manifest written a moment ago is exactly what they are looking for).
 *
 * A failed read never rejects: it lands on the entry's `error`, because a sidebar
 * row must keep rendering and a card must keep its status while the extension's
 * manifests are unreadable.
 */
export async function loadSubagentChildren(
  workspaceId: number,
  parentSessionIds: (string | number)[],
  options: { force?: boolean } = {},
): Promise<void> {
  const force = options.force === true;
  const keys = [...new Set(parentSessionIds.map((id) => String(id)))];
  await Promise.all(keys.map((key) => loadOne(workspaceId, key, force)));
}

/**
 * Reads the children of every chat in one workspace's list, and forgets the ones
 * whose chat is no longer there.
 *
 * This is the sidebar's entry point: the list it just read *is* the set of visible
 * chats, so a parent that disappeared (a deleted session) must take its cached
 * children with it instead of leaving an orphan block under nothing.
 */
export async function syncWorkspaceSubagents(
  workspaceId: number,
  parentSessionIds: (string | number)[],
): Promise<void> {
  const keys = new Set(parentSessionIds.map((id) => String(id)));
  for (const [parentSessionId, entry] of [...parents.entries()]) {
    if (entry.workspaceId !== workspaceId) continue;
    if (!keys.has(parentSessionId)) parents.delete(parentSessionId);
  }
  await loadSubagentChildren(workspaceId, [...keys], { force: true });
}

// ── the read-only view (S3) ──────────────────────────────────────────────────

/**
 * Paints the child's transcript into the shape the turn-group renderer takes.
 *
 * pi's lines carry no id (and no reasoning: the JSONL never stored it for us), so
 * each one gets a handle here — derived from the child and its position, which is
 * stable across refreshes because a transcript only grows at the end.
 */
function toChildTranscript(
  childSessionId: number,
  history: TranscriptMessage[],
): ChatMessage[] {
  return history.map((message, index) => ({
    id: `subagent:${childSessionId}:${index}`,
    role: message.role,
    text: message.text,
    at: message.at,
  }));
}

/** Whether two reads describe the same transcript (a poll mostly repeats itself). */
function sameTranscript(current: ChatMessage[], next: ChatMessage[]): boolean {
  if (current.length !== next.length) return false;
  const lastCurrent = current[current.length - 1];
  const lastNext = next[next.length - 1];
  if (lastCurrent === undefined || lastNext === undefined) return true;
  return lastCurrent.text === lastNext.text && lastCurrent.at === lastNext.at;
}

/**
 * Re-reads the file of the child on screen. This is the whole "live" part of the
 * view: pi's child writes its own JSONL as it works, so following the file is
 * enough to watch it — no protocol, no event stream for a session Nabla does not
 * own (S3).
 */
async function refreshSubagentTranscript(): Promise<void> {
  const open = openSubagent.value;
  if (open === null) return;
  if (subagentTranscript.value.length === 0) subagentTranscriptLoading.value = true;
  try {
    const history = await sessionTranscript(String(open.child.sessionId));
    if (openSubagent.value?.child.sessionId !== open.child.sessionId) return;
    const next = toChildTranscript(open.child.sessionId, history);
    if (!sameTranscript(subagentTranscript.value, next)) {
      subagentTranscript.value = next;
    }
    subagentTranscriptError.value = null;
  } catch (cause) {
    if (openSubagent.value?.child.sessionId !== open.child.sessionId) return;
    subagentTranscriptError.value = commandErrorMessage(cause);
  } finally {
    // Only the read of the child still on screen may say it finished loading: a
    // second view opened in the meantime owns that flag now.
    if (openSubagent.value?.child.sessionId === open.child.sessionId) {
      subagentTranscriptLoading.value = false;
    }
  }
}

/**
 * Opens the read-only view of one child. Nothing is spawned and nothing is
 * stopped: the parent chat keeps its process, its buffer and its place in the
 * sidebar, and this only decides what the chat column renders.
 */
export function openSubagentView(
  child: SubagentSession,
  parentSessionId: string,
  workspaceId: number,
): void {
  openSubagent.value = { child, parentSessionId, workspaceId };
  subagentTranscript.value = [];
  subagentTranscriptError.value = null;
  subagentTranscriptLoading.value = true;
  ensureParentEntry(workspaceId, parentSessionId);
  void refreshSubagentTranscript();
  // The status shown next to the header comes from the parent's list, so read it
  // now instead of waiting for the sidebar's next tick.
  void loadOne(workspaceId, parentSessionId, true);
}

/**
 * Returns the panel to the chat. The parent chat is not re-selected and no process
 * is touched: it was never left, only covered (S3/S4).
 */
export function closeSubagentView(): void {
  openSubagent.value = null;
  subagentTranscript.value = [];
  subagentTranscriptError.value = null;
  subagentTranscriptLoading.value = false;
  syncPolling();
}

// ── the one interval (H4.1) ──────────────────────────────────────────────────

/** Is any known child (or the one on screen) working right now? */
function needsPolling(): boolean {
  for (const entry of parents.values()) {
    if (entry.children.some((child) => isSubagentActive(child.status))) return true;
  }
  return false;
}

/**
 * Follows the state: an interval exists exactly while some child is running, and
 * a run that ends (or a view that closes) stops it on the spot. Nothing here polls
 * a chat whose children are all done — the whole point of the reviewed status is
 * that an idle sidebar costs nothing.
 */
function syncPolling(): void {
  if (!needsPolling()) {
    stopPolling();
    return;
  }
  if (pollHandle !== null) return;
  pollHandle = window.setInterval(() => {
    void pollRunningChildren();
  }, POLL_MS);
}

function stopPolling(): void {
  if (pollHandle === null) return;
  window.clearInterval(pollHandle);
  pollHandle = null;
}

/**
 * One tick: re-asks every parent that has a running child and, when the child on
 * screen is the running one, re-reads its file so the view grows with it. A hidden
 * document is skipped — nobody is watching, and coming back refreshes on the next
 * tick anyway.
 */
async function pollRunningChildren(): Promise<void> {
  if (!needsPolling()) {
    stopPolling();
    return;
  }
  if (typeof document !== "undefined" && document.visibilityState !== "visible") return;
  const tasks: Promise<void>[] = [];
  for (const [parentSessionId, entry] of [...parents.entries()]) {
    if (!entry.children.some((child) => isSubagentActive(child.status))) continue;
    tasks.push(loadOne(entry.workspaceId, parentSessionId, true));
  }
  if (openSubagent.value !== null && isSubagentActive(openSubagentStatus.value)) {
    tasks.push(refreshSubagentTranscript());
  }
  await Promise.all(tasks);
}

/**
 * Vite replaces this module on every edit and the interval would outlive the copy
 * that started it — one more timer per reload, all of them polling. Disposing keeps
 * "one interval per app", including in development.
 */
if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    stopPolling();
  });
}
