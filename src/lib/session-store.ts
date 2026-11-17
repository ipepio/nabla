// Shared session state (tasks L3/L4 of `odd/tasks/session-lifecycle.md`).
//
// The drawer and the chat panel both need the same answers — which session is
// active, what each session has said, and which ones have a live pi process —
// so the state lives here at module scope: one instance per app, one event
// subscription, every importer reading and writing the same objects.
//
// S4 is the rule this module exists for: switching sessions is a *view* change.
// Nothing here stops, restarts or recreates a process. Each session keeps its
// own buffer, and the single `agent-event` subscription routes every frame by
// `payload.sessionId` — including frames for sessions that are not on screen —
// so a session that lost the focus keeps accumulating its transcript.
//
// S5 falls out of the same shape: `open_session` is idempotent, so a session
// that already runs is reused, never duplicated.
import { computed, reactive, ref } from "vue";
import {
  commandErrorMessage,
  liveSessionIds,
  openSession,
  sendPrompt,
  sessionTranscript,
  startSession,
  subscribeAgentEvents,
} from "../contracts/commands";
import type { AgentEvent, RuntimeStatus } from "../contracts/agent-events";
import type {
  SessionDto,
  SessionSummary,
  TranscriptMessage,
  TranscriptRole,
} from "../contracts/types";
import { queueMessages } from "./preferences";

/** One tool call of a session, kept as the compact chip the panel renders. */
export interface ToolEntry {
  toolCallId: string;
  toolName: string;
  state: "running" | "done" | "error";
}

/**
 * One completed entry of a session's transcript, as this UI renders it.
 *
 * It extends pi's own line (`TranscriptMessage`, read back from the JSONL file)
 * with UI-only state: `id` is this store's stable handle on the message and
 * `reasoning` is what pi streamed as `thinking_delta` for the same turn. Neither
 * crosses the contracts seam — the Rust DTOs stay exactly as pi wrote them.
 */
export interface ChatMessage {
  /** Stable for as long as the store lives; keys the reasoning open/closed state. */
  id: string;
  role: TranscriptRole;
  text: string;
  at: string | null;
  /** The turn's reasoning, absent when pi streamed none. */
  reasoning?: string;
}

/**
 * Id of the next message. Prefixed with the session id so a deleted session's
 * bookkeeping (the reasoning toggle state) can be pruned by prefix, and
 * monotonic so an id is never reused inside one app run.
 */
let messageSequence = 0;
function nextMessageId(sessionId: string): string {
  messageSequence += 1;
  return `${sessionId}:${messageSequence}`;
}

/**
 * Everything one session has accumulated. One of these exists per session the
 * store has seen, whether or not it is on screen: that is what makes S4 work.
 */
export interface SessionBuffer {
  /** Completed turns, oldest first: seeded from pi's file, then appended live. */
  transcript: ChatMessage[];
  /** The assistant turn being streamed right now; flushed into `transcript`. */
  streamingText: string;
  /**
   * The reasoning pi is streaming for the turn in progress (`thinking_delta`).
   * It is folded into the message `flushStreaming` writes and then cleared: the
   * live field only ever holds the current turn, so a session switch shows a
   * completed turn's reasoning from the transcript and never from here.
   */
  streamingReasoning: string;
  /**
   * Id of the message being streamed. Minted on the turn's first delta and
   * reused by the message it flushes into, so the reasoning block a human opens
   * while it streams stays open once the turn lands in the transcript.
   */
  liveMessageId: string | null;
  tools: ToolEntry[];
  status: RuntimeStatus | null;
  /** Runtime diagnosis (a crash's stderr tail) or the last command failure. */
  error: string | null;
  /**
   * Exit code of the last `process_exit`, so the status line can say
   * "pi is stopped (exit code N)" as it did before the buffers existed.
   */
  exitCode: number | null;
  /**
   * The agent is doing work for this session right now (streaming a reply or
   * running a tool). `turn_end`/`agent_end` clear it, so the drawer can show a
   * spinner on the row that is actually busy — independent of whether the
   * process merely exists.
   */
  busy: boolean;
  /**
   * Owning workspace, remembered when the session is created, opened, seeded or
   * enqueued into. `send_prompt` takes the pair, so a queued prompt cannot be
   * sent without it; `null` for a session the store only learned about from an
   * event. While it is `null` the queue waits instead of being dropped.
   */
  workspaceId: number | null;
  /**
   * S3 of `chat-preferences-and-queue.md`: prompts the human sent while this
   * session's agent was working, oldest first. Per session, never global — a
   * queue belongs to the conversation it was typed into. One entry is shifted
   * and sent each time a turn ends, so it drains in order. A queue left over
   * after the toggle is turned off simply waits and drains on the next finished
   * turn once it is back on, and so does one whose session cannot be addressed
   * yet: an entry leaves this list only when it is really sent.
   */
  queue: string[];
}

/** Per-session state, keyed by the session index id in decimal form (S1). */
export const sessions = reactive(new Map<string, SessionBuffer>());

/** The session the chat panel follows, or `null` before the first selection. */
export const activeSession = ref<SessionDto | null>(null);

/** Id of the active session (S1: the row id, never the workspace). */
export const activeSessionId = computed<string | null>(
  () => activeSession.value?.sessionId ?? null,
);

/**
 * L4: ids with a live process, as last reported by the runtime. The drawer marks
 * them; nothing here infers liveness, it only mirrors `live_session_ids` and the
 * lifecycle events.
 */
export const liveIds = ref<string[]>([]);

/**
 * Ids whose agent is currently doing work. Derived from the buffers, so the
 * drawer shows a spinner on the row that is actually producing output.
 */
export const busyIds = computed<string[]>(() =>
  [...sessions.entries()]
    .filter(([, buffer]) => buffer.busy)
    .map(([sessionId]) => sessionId),
);

/**
 * Open/closed state of the reasoning blocks the human toggled, keyed by message
 * id. Collapsed is the default (pi's own setting keeps reasoning hidden), so an
 * entry exists only after an explicit toggle and survives re-renders and session
 * switches for as long as the session lives in this store.
 */
const reasoningExpanded = reactive(new Map<string, boolean>());

/** Whether a message's reasoning block is open. Collapsed until toggled. */
export function isReasoningExpanded(messageId: string): boolean {
  return reasoningExpanded.get(messageId) === true;
}

/** Flips one message's reasoning block; the state is remembered here, not in a
 * component, so switching sessions does not collapse it. */
export function toggleReasoning(messageId: string): void {
  reasoningExpanded.set(messageId, !isReasoningExpanded(messageId));
}

/** Sessions whose transcript is being read from pi's file (S3, first open). */
const transcriptLoading = reactive(new Set<string>());

/**
 * Sessions whose transcript has already been read. Kept beside the buffers (not
 * inside them) so the buffer stays exactly the shape the panel renders and a
 * refresh of the same session does not re-read the file.
 */
const transcriptSeeded = new Set<string>();

/**
 * Bumped whenever a session is created. The drawer watches it to refresh the
 * index lists, which is the only thing it means.
 */
export const sessionGeneration = ref(0);

/** The buffer of one session, or `null` when the store has never seen it. */
export function sessionBuffer(sessionId: string | null): SessionBuffer | null {
  if (sessionId === null) return null;
  return sessions.get(sessionId) ?? null;
}

/** S3: the transcript of a session that is being read for the first time. */
export function isTranscriptLoading(sessionId: string | null): boolean {
  if (sessionId === null) return false;
  return transcriptLoading.has(sessionId);
}

// ── live-process bookkeeping (L4) ────────────────────────────────────────────

function isLive(status: RuntimeStatus): boolean {
  return status === "spawning" || status === "ready" || status === "running";
}

function markLive(sessionId: string, status: RuntimeStatus): void {
  const live = isLive(status);
  const known = liveIds.value.includes(sessionId);
  if (live === known) return;
  liveIds.value = live
    ? [...liveIds.value, sessionId]
    : liveIds.value.filter((id) => id !== sessionId);
}

/**
 * L4: re-reads the liveness the runtime reports. Failures keep the previous list
 * instead of claiming every session stopped: this is a hint on a list row, not a
 * decision, and it must never turn an unknown answer into "nothing is running".
 */
export async function refreshLiveSessions(): Promise<string[]> {
  try {
    const ids = await liveSessionIds();
    liveIds.value = ids;
    return ids;
  } catch {
    return liveIds.value;
  }
}

// ── the single event subscription (S4) ───────────────────────────────────────

let eventsInstalled = false;
let eventsInstalling: Promise<void> | null = null;

/**
 * Installs the one `agent-event` subscription this app has. Called on first use
 * (a selection or the panel's mount) and never from more than one place; later
 * callers join the same promise, so no component owns a listener and none can
 * unsubscribe the others.
 *
 * It never rejects: a bridge that is not up yet leaves the flag down, so the next
 * caller retries instead of the app staying deaf for the rest of the session.
 */
export function ensureSessionEvents(): Promise<void> {
  if (eventsInstalled) return Promise.resolve();
  if (eventsInstalling === null) {
    eventsInstalling = subscribeAgentEvents((envelope) => {
      // The envelope carries the session id (S1): route by it, not by what is on
      // screen, so a background session keeps its transcript (S4).
      applyEvent(envelope.sessionId, envelope.event);
    })
      .then(() => {
        eventsInstalled = true;
      })
      .catch(() => {
        eventsInstalling = null;
      });
  }
  return eventsInstalling;
}

/**
 * Creates the buffer for a session the store has not seen yet.
 *
 * `workspaceId` is optional because an event can arrive for a session this store
 * never opened; a later call that does know the workspace fills it in, so the
 * queue of a session learned from an event still becomes sendable.
 */
function ensureBuffer(sessionId: string, workspaceId: number | null = null): SessionBuffer {
  const existing = sessions.get(sessionId);
  if (existing !== undefined) {
    if (existing.workspaceId === null && workspaceId !== null) {
      existing.workspaceId = workspaceId;
    }
    return existing;
  }
  // `reactive` here keeps the proxy identity stable: the map stores the raw
  // target and hands back this same proxy, so callers may hold either.
  const created = reactive<SessionBuffer>({
    transcript: [],
    streamingText: "",
    streamingReasoning: "",
    liveMessageId: null,
    tools: [],
    status: null,
    error: null,
    exitCode: null,
    busy: false,
    workspaceId,
    queue: [],
  });
  sessions.set(sessionId, created);
  return created;
}

/**
 * The message id of the turn in progress, minting it on the turn's first delta.
 * One id per turn, shared by the live view and by whatever it flushes into.
 */
function ensureLiveMessageId(sessionId: string, buffer: SessionBuffer): string {
  buffer.liveMessageId ??= nextMessageId(sessionId);
  return buffer.liveMessageId;
}

/**
 * Moves the streamed assistant turn into the completed transcript: text and
 * reasoning land as one message, and both live fields are emptied so the next
 * turn starts clean in every session that stayed open off screen.
 */
function flushStreaming(sessionId: string, buffer: SessionBuffer): void {
  const text = buffer.streamingText;
  const reasoning = buffer.streamingReasoning;
  if (text === "" && reasoning === "") return;
  buffer.streamingText = "";
  buffer.streamingReasoning = "";
  // The live id becomes the message's id, so an open reasoning block stays open.
  const id = buffer.liveMessageId ?? nextMessageId(sessionId);
  buffer.liveMessageId = null;
  const message: ChatMessage = {
    id,
    role: "assistant",
    text,
    at: new Date().toISOString(),
  };
  if (reasoning !== "") message.reasoning = reasoning;
  buffer.transcript.push(message);
}

// ── per-session prompt queue (S3) ────────────────────────────────────────────

/**
 * S3: adds one prompt to the session's queue. ChatPanel calls this instead of
 * `sendPrompt` when the composer's submit arrives while the agent is working, so
 * the prompt is kept rather than rejected or sent into a busy session.
 *
 * `workspaceId` is the workspace the caller already knows (the panel always
 * does: it renders a session that belongs to one). Recording it here is what
 * makes the queued prompt addressable later, when the panel may have moved to
 * another workspace or session entirely.
 *
 * Nothing is echoed into the transcript here: the turn appears when it is really
 * sent (see `sendQueuedTurn`), so a dropped item never leaves a phantom message.
 */
export function enqueuePrompt(
  sessionId: string,
  text: string,
  workspaceId: number | null = null,
): void {
  const prompt = text.trim();
  if (prompt === "") return;
  ensureBuffer(sessionId, workspaceId).queue.push(prompt);
}

/** How many prompts of one session are waiting; `0` when there are none. */
export function queuedCount(sessionId: string | null): number {
  if (sessionId === null) return 0;
  return sessions.get(sessionId)?.queue.length ?? 0;
}

/**
 * S3: the prompts of one session that are still waiting, oldest first.
 *
 * A copy, not the buffer's own array: the panel renders the list but only
 * `removeQueuedPrompt` may change it. Reading the reactive queue here is what
 * keeps the panel up to date — a computed over this function re-evaluates
 * whenever an entry is added, sent or removed.
 */
export function queuedPrompts(sessionId: string | null): string[] {
  const buffer = sessionBuffer(sessionId);
  if (buffer === null) return [];
  return [...buffer.queue];
}

/**
 * S3: drops one waiting prompt of a session by position. The human's own undo
 * for a queued item; out-of-range indices are ignored instead of throwing, since
 * the list can shrink on its own when a turn ends and drains it.
 */
export function removeQueuedPrompt(
  sessionId: string | null,
  index: number,
): void {
  const buffer = sessionBuffer(sessionId);
  if (buffer === null) return;
  if (!Number.isInteger(index) || index < 0 || index >= buffer.queue.length) return;
  // `splice` on the reactive array: the queue is mutated in place so every
  // computed watching it (the counter, the list) updates.
  buffer.queue.splice(index, 1);
}

/**
 * S3: hands the next queued prompt of a session to pi, after the turn that just
 * ended. Exactly one per turn end, so a queue of three takes three turns — same
 * order the human typed them.
 *
 * Addressability comes first. `send_prompt` takes the (workspace, session) pair,
 * so a session whose workspace is unknown cannot be sent to; the queue is left
 * exactly as it is then, and its first entry goes out on a later turn, once a
 * path that knows the workspace has recorded it. Shifting before that check
 * destroyed the prompt instead — it was reported as sent and never processed.
 * Nothing is written to `buffer.error` either: a workspace that is merely not
 * known yet is not a failure to show the human.
 */
function drainQueue(sessionId: string, buffer: SessionBuffer): void {
  if (!queueMessages.value) return;
  const workspaceId = buffer.workspaceId;
  if (workspaceId === null) return;
  const next = buffer.queue.shift();
  if (next === undefined) return;
  void sendQueuedTurn(sessionId, buffer, workspaceId, next);
}

/**
 * Sends one dequeued prompt and records it as a human turn, exactly as the
 * composer's own send path does (pi answers with deltas, it never echoes the
 * prompt back). A failure lands on the session's error field, which the panel
 * already renders, instead of being swallowed or thrown into the event loop.
 */
async function sendQueuedTurn(
  sessionId: string,
  buffer: SessionBuffer,
  workspaceId: number,
  text: string,
): Promise<void> {
  try {
    await sendPrompt(workspaceId, Number(sessionId), text);
    appendUserTurn(sessionId, text);
    // The drawer sorts by activity, so a queued turn is activity too.
    sessionGeneration.value += 1;
  } catch (cause) {
    buffer.error = commandErrorMessage(cause);
  }
}

function findTool(buffer: SessionBuffer, toolCallId: string) {
  return buffer.tools.find((entry) => entry.toolCallId === toolCallId);
}

/** Applies one forwarded pi event to the buffer of the session it belongs to. */
function applyEvent(sessionId: string, event: AgentEvent): void {
  const buffer = ensureBuffer(sessionId);
  // Busy is "the agent is working for this session", not "a process exists":
  // streaming and tool activity raise it, an ended turn lowers it. Events keep
  // arriving for off-screen sessions, so this stays correct for the whole list.
  if (
    event.type === "message_update" ||
    event.type === "tool_execution_start" ||
    event.type === "tool_execution_update"
  ) {
    buffer.busy = true;
  } else if (
    event.type === "turn_end" ||
    event.type === "agent_end" ||
    event.type === "process_exit"
  ) {
    buffer.busy = false;
  }
  switch (event.type) {
    case "message_update": {
      const slice = event.assistantMessageEvent;
      const delta = slice.delta;
      if (delta !== undefined && delta !== "") {
        // Both kinds of delta grow the same turn; the kind only decides whether
        // the text is the answer or the reasoning kept behind its own block.
        if (slice.type === "text_delta" || slice.type === "thinking_delta") {
          ensureLiveMessageId(sessionId, buffer);
          if (slice.type === "thinking_delta") buffer.streamingReasoning += delta;
          else buffer.streamingText += delta;
        }
      } else if (slice.type === "done" || slice.type === "error") {
        // The turn is complete on pi's side; the growing tail becomes a message.
        flushStreaming(sessionId, buffer);
      }
      return;
    }
    case "tool_execution_start":
      buffer.tools.push({
        toolCallId: event.toolCallId,
        toolName: event.toolName,
        state: "running",
      });
      return;
    case "tool_execution_update": {
      const entry = findTool(buffer, event.toolCallId);
      if (entry === undefined) {
        buffer.tools.push({
          toolCallId: event.toolCallId,
          toolName: event.toolName,
          state: "running",
        });
      } else {
        entry.state = "running";
      }
      return;
    }
    case "tool_execution_end": {
      const entry = findTool(buffer, event.toolCallId);
      if (entry === undefined) {
        buffer.tools.push({
          toolCallId: event.toolCallId,
          toolName: event.toolName,
          state: event.isError ? "error" : "done",
        });
      } else {
        entry.state = event.isError ? "error" : "done";
      }
      return;
    }
    case "turn_end":
    case "agent_end":
      // Belt to the `done` slice above: a stream that ends without one still
      // lands in the transcript instead of vanishing.
      flushStreaming(sessionId, buffer);
      // A finished turn is activity the drawer must see: bump the generation so
      // the session list re-reads and the row moves up now, instead of waiting
      // for the next poll tick.
      sessionGeneration.value += 1;
      // S3: the finished turn is the only moment a queued prompt is sent, so
      // nothing ever talks to pi while it is working. Flushing first keeps the
      // conversation in order: the answer lands above the next question.
      drainQueue(sessionId, buffer);
      return;
    case "process_status":
      buffer.status = event.status;
      buffer.error = event.error ?? null;
      markLive(sessionId, event.status);
      return;
    case "process_exit":
      buffer.exitCode = event.code ?? null;
      liveIds.value = liveIds.value.filter((id) => id !== sessionId);
      return;
    default:
      // agent_start/turn_start and unmodelled pi events add nothing here; the
      // stream keeps flowing past them.
      return;
  }
}

// ── lifecycle ────────────────────────────────────────────────────────────────

/** The identity the panel can bind to before the runtime has answered. */
function dtoFromSummary(
  session: SessionSummary,
  status: RuntimeStatus | null,
): SessionDto {
  return {
    sessionId: String(session.id),
    workspaceId: session.workspaceId,
    piSessionPath: session.piSessionPath,
    // Nothing claims a live process: `pid`/`status` come from the runtime the
    // moment it answers, and the panel renders the buffer's status.
    pid: 0,
    status: status ?? "stopped",
  };
}

/** Seeds a buffer with the history pi already has on disk (S3). */
function seedTranscript(
  sessionId: string,
  buffer: SessionBuffer,
  history: TranscriptMessage[],
): void {
  if (history.length === 0) return;
  // pi's lines carry no id and no reasoning (it never stored them for us), so
  // each one gets a handle here: the panel needs a stable key per message.
  const seeded: ChatMessage[] = history.map((message) => ({
    id: nextMessageId(sessionId),
    role: message.role,
    text: message.text,
    at: message.at,
  }));
  // History goes in front: anything already in the buffer arrived live while
  // the file was being read and is newer than what the file holds.
  buffer.transcript = [...seeded, ...buffer.transcript];
}

/**
 * S3/S5: makes a session the one the chat shows.
 *
 * Switching is instant and side-effect free towards the previous session: the
 * panel binds to this session's existing buffer, nothing is stopped (S4). The
 * process is opened only when the runtime says there is none, and an already
 * live session is reused through the idempotent `open_session` (S5). The first
 * time a session is selected its transcript is read from pi's file (S3).
 */
export async function selectSession(session: SessionSummary): Promise<void> {
  const sessionId = String(session.id);
  // S3: the workspace travels with the buffer, so a prompt queued in this
  // session can be resent even if the panel has moved on by then.
  const buffer = ensureBuffer(sessionId, session.workspaceId);
  activeSession.value = dtoFromSummary(session, buffer.status);
  void ensureSessionEvents();

  const live = await refreshLiveSessions();
  if (!live.includes(sessionId)) {
    try {
      const opened = await openSession(sessionId);
      // The runtime's answer carries the workspace too; recording it here keeps
      // the open path addressable even for a session the caller described
      // without one.
      ensureBuffer(sessionId, opened.workspaceId);
      buffer.status = opened.status;
      buffer.error = null;
      markLive(sessionId, opened.status);
      // The runtime's answer is better than the placeholder; only replace the
      // identity if the human has not moved on to another session meanwhile.
      if (activeSession.value?.sessionId === sessionId) activeSession.value = opened;
    } catch (cause) {
      buffer.error = commandErrorMessage(cause);
    }
  }

  if (transcriptSeeded.has(sessionId)) return;
  transcriptLoading.add(sessionId);
  try {
    const history = await sessionTranscript(sessionId);
    seedTranscript(sessionId, buffer, history);
    transcriptSeeded.add(sessionId);
  } catch (cause) {
    // Not fatal: the session still streams, and the next selection retries.
    buffer.error = buffer.error ?? commandErrorMessage(cause);
  } finally {
    transcriptLoading.delete(sessionId);
  }
}

/**
 * S2: creates a session for a workspace. This is the only creation path — the
 * drawer's button and the composer's first send both land here, so pi is never
 * spawned twice for one intent.
 *
 * Failures propagate to the caller: each caller owns the surface it renders the
 * message on, and this module never invents UI state.
 */
export async function newSession(workspaceId: number): Promise<SessionDto> {
  const started = await startSession(workspaceId);
  const buffer = ensureBuffer(started.sessionId, workspaceId);
  buffer.status = started.status;
  buffer.error = null;
  markLive(started.sessionId, started.status);
  // A session created here has no earlier conversation to read back.
  transcriptSeeded.add(started.sessionId);
  activeSession.value = started;
  sessionGeneration.value += 1;
  return started;
}

/** Drops the active session, e.g. before the chat binds to a new workspace. */
export function clearSession(): void {
  if (activeSession.value === null) return;
  activeSession.value = null;
  sessionGeneration.value += 1;
}

/**
 * Forgets a session the app just deleted: its buffer, its seeded/loading marks
 * and its liveness entry go with it, so a deleted session can never come back as
 * a stale row in the chat. When it was the active session the chat is left with
 * nothing bound, which is the honest state — the row no longer exists.
 */
export function removeSession(sessionId: string | number): void {
  const key = String(sessionId);
  sessions.delete(key);
  // The toggle state of a session that no longer exists has nothing left to
  // open: message ids are prefixed with their session, so they prune by prefix.
  for (const messageId of [...reasoningExpanded.keys()]) {
    if (messageId.startsWith(`${key}:`)) reasoningExpanded.delete(messageId);
  }
  transcriptSeeded.delete(key);
  transcriptLoading.delete(key);
  liveIds.value = liveIds.value.filter((id) => id !== key);
  if (activeSession.value?.sessionId === key) activeSession.value = null;
}

/**
 * Records a turn the human just sent. pi answers with deltas, it does not echo
 * the prompt back, so the store is what makes the transcript read as a
 * conversation.
 */
export function appendUserTurn(sessionId: string, text: string): void {
  const buffer = ensureBuffer(sessionId);
  buffer.transcript.push({
    id: nextMessageId(sessionId),
    role: "user",
    text,
    at: new Date().toISOString(),
  });
}

/**
 * Confirms a lifecycle transition the user asked for (an explicit stop) with the
 * runtime's own answer instead of assuming the transition landed.
 */
export function recordSessionStatus(
  sessionId: string,
  status: RuntimeStatus,
  error: string | null,
): void {
  const buffer = ensureBuffer(sessionId);
  buffer.status = status;
  buffer.error = error;
  markLive(sessionId, status);
}
