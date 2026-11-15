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
  sessionTranscript,
  startSession,
  subscribeAgentEvents,
} from "../contracts/commands";
import type { AgentEvent, RuntimeStatus } from "../contracts/agent-events";
import type { SessionDto, SessionSummary, TranscriptMessage } from "../contracts/types";

/** One tool call of a session, kept as the compact chip the panel renders. */
export interface ToolEntry {
  toolCallId: string;
  toolName: string;
  state: "running" | "done" | "error";
}

/**
 * Everything one session has accumulated. One of these exists per session the
 * store has seen, whether or not it is on screen: that is what makes S4 work.
 */
export interface SessionBuffer {
  /** Completed turns, oldest first: seeded from pi's file, then appended live. */
  transcript: TranscriptMessage[];
  /** The assistant turn being streamed right now; flushed into `transcript`. */
  streamingText: string;
  tools: ToolEntry[];
  status: RuntimeStatus | null;
  /** Runtime diagnosis (a crash's stderr tail) or the last command failure. */
  error: string | null;
  /**
   * Exit code of the last `process_exit`, so the status line can say
   * "pi is stopped (exit code N)" as it did before the buffers existed.
   */
  exitCode: number | null;
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

/** Creates the buffer for a session the store has not seen yet. */
function ensureBuffer(sessionId: string): SessionBuffer {
  const existing = sessions.get(sessionId);
  if (existing !== undefined) return existing;
  // `reactive` here keeps the proxy identity stable: the map stores the raw
  // target and hands back this same proxy, so callers may hold either.
  const created = reactive<SessionBuffer>({
    transcript: [],
    streamingText: "",
    tools: [],
    status: null,
    error: null,
    exitCode: null,
  });
  sessions.set(sessionId, created);
  return created;
}

/** Moves the streamed assistant text into the completed transcript. */
function flushStreaming(buffer: SessionBuffer): void {
  const text = buffer.streamingText;
  if (text === "") return;
  buffer.streamingText = "";
  buffer.transcript.push({ role: "assistant", text, at: new Date().toISOString() });
}

function findTool(buffer: SessionBuffer, toolCallId: string) {
  return buffer.tools.find((entry) => entry.toolCallId === toolCallId);
}

/** Applies one forwarded pi event to the buffer of the session it belongs to. */
function applyEvent(sessionId: string, event: AgentEvent): void {
  const buffer = ensureBuffer(sessionId);
  switch (event.type) {
    case "message_update": {
      const slice = event.assistantMessageEvent;
      if (slice.type === "text_delta" && slice.delta !== undefined && slice.delta !== "") {
        buffer.streamingText += slice.delta;
      } else if (slice.type === "done" || slice.type === "error") {
        // The turn is complete on pi's side; the growing tail becomes a message.
        flushStreaming(buffer);
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
      flushStreaming(buffer);
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
function seedTranscript(buffer: SessionBuffer, history: TranscriptMessage[]): void {
  if (history.length === 0) return;
  // History goes in front: anything already in the buffer arrived live while
  // the file was being read and is newer than what the file holds.
  buffer.transcript = [...history, ...buffer.transcript];
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
  const buffer = ensureBuffer(sessionId);
  activeSession.value = dtoFromSummary(session, buffer.status);
  void ensureSessionEvents();

  const live = await refreshLiveSessions();
  if (!live.includes(sessionId)) {
    try {
      const opened = await openSession(sessionId);
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
    seedTranscript(buffer, history);
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
  const buffer = ensureBuffer(started.sessionId);
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
