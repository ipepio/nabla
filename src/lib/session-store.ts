// Shared active-session state (task N1 of `odd/tasks/sidebar-rework.md`).
//
// Until now the started session lived inside `ChatPanel`. Task N2 gives the
// drawer its own "New session" button and its own session list, so two
// components now create and select sessions — and both must agree on which one
// the chat is bound to. The state therefore moves to module scope: this module
// is the single instance, and every importer reads and writes the same refs.
//
// The surface is deliberately small. Prompt text, streaming flags, tool events
// and status stay in `ChatPanel` (they are rendering state, not identity); this
// module owns only *which* session is active and how a new one is created.
import { ref } from "vue";
import { startSession } from "../contracts/commands";
import type { SessionDto, SessionSummary } from "../contracts/types";

/** The one session the chat follows, or `null` before the first start. */
export const activeSession = ref<SessionDto | null>(null);

/**
 * Bumped whenever the active session is created, opened or cleared. Consumers
 * watch this counter rather than the DTO: a start, an open and a clear are all
 * "the view must reset", and comparing DTOs would miss the case where the same
 * row is selected twice.
 */
export const sessionGeneration = ref(0);

/**
 * S2: creates the session for a workspace. This is the only creation path —
 * the drawer's button and the composer's "first send starts it" both land here,
 * so pi is never spawned twice for one intent.
 *
 * Failures propagate to the caller: each caller owns the surface it renders the
 * message on, and this module never invents UI state.
 */
export async function newSession(workspaceId: number): Promise<SessionDto> {
  const started = await startSession(workspaceId);
  activeSession.value = started;
  sessionGeneration.value += 1;
  return started;
}

/**
 * Follows an already-registered session picked from the drawer's list (S5).
 *
 * The index row carries no live process facts, so `pid`/`status` are the
 * neutral placeholders `0`/`stopped`: nothing claims the process is running,
 * and the first `process_status` event from the runtime replaces them. Opening
 * a session never spawns pi — `start_session` is what does that.
 */
export function openSession(session: SessionSummary): void {
  activeSession.value = {
    sessionId: String(session.id),
    workspaceId: session.workspaceId,
    piSessionPath: session.piSessionPath,
    pid: 0,
    status: "stopped",
  };
  sessionGeneration.value += 1;
}

/** Drops the active session, e.g. before the chat binds to a new workspace. */
export function clearSession(): void {
  if (activeSession.value === null) return;
  activeSession.value = null;
  sessionGeneration.value += 1;
}
