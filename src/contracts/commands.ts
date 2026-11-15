// Typed wrappers around the Tauri commands (spec S4).
// Vue never talks to pi directly: every read and write goes through Rust.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AgentEventEnvelope } from "./agent-events";
import type {
  AppSettings,
  ModelInfo,
  ModelOption,
  ProviderDraft,
  ProviderSummary,
  SessionDto,
  SessionStatusDto,
  SessionSummary,
  TranscriptMessage,
  WorkspaceInfo,
} from "./types";

/**
 * Tauri commands reject with a plain string; normalize any rejection into a
 * message that is safe to render in the UI.
 */
export function commandErrorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Unexpected bridge error.";
}

export function listWorkspaces(): Promise<WorkspaceInfo[]> {
  return invoke<WorkspaceInfo[]>("list_workspaces");
}

export function addWorkspace(
  path: string,
  name?: string | null,
): Promise<WorkspaceInfo> {
  return invoke<WorkspaceInfo>("add_workspace", { path, name: name ?? null });
}

export function removeWorkspace(id: number): Promise<boolean> {
  return invoke<boolean>("remove_workspace", { id });
}

/**
 * S8: answers the same payload as `list_workspaces`, so the drawer can refresh
 * from one source of truth instead of patching its own copy.
 */
export function setWorkspacePinned(
  workspaceId: number,
  pinned: boolean,
): Promise<WorkspaceInfo> {
  return invoke<WorkspaceInfo>("set_workspace_pinned", { workspaceId, pinned });
}

/**
 * S8: records that the space was just used. The recency timestamp is the store's
 * clock, never the webview's.
 */
export function touchWorkspace(workspaceId: number): Promise<WorkspaceInfo> {
  return invoke<WorkspaceInfo>("touch_workspace", { workspaceId });
}

/**
 * S7: the models the configured provider offers. The list lives in Rust (one
 * constant, sourced from pi's config); the UI never hardcodes model ids.
 */
export function listModels(): Promise<ModelInfo[]> {
  return invoke<ModelInfo[]>("list_models");
}

export function listSessions(workspaceId: number): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>("list_sessions", { workspaceId });
}

export function getSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_settings");
}

export function saveSettings(settings: AppSettings): Promise<AppSettings> {
  return invoke<AppSettings>("save_settings", { settings });
}

/**
 * S2 of `settings-dialog.md`: the absolute path of the preferences file the app
 * reads and writes. Rust resolves it, so the dialog never guesses a directory.
 */
export function configFilePath(): Promise<string> {
  return invoke<string>("config_file_path");
}

/**
 * S2: opens the preferences file with the system's default editor. Rust creates
 * it with the defaults first when it does not exist yet, so the action always
 * opens a real file.
 */
export function openConfigFile(): Promise<void> {
  return invoke<void>("open_config_file");
}

// ── providers seam (S1-S3 of `providers-pane.md`) ───────────────────────────

/**
 * S1: the providers pi knows, read from `~/.pi/agent/models.json`. The api key is
 * reduced to `hasApiKey` on the Rust side (S4), so it never reaches the webview.
 */
export function listProviders(): Promise<ProviderSummary[]> {
  return invoke<ProviderSummary[]>("list_providers");
}

/**
 * S3: writes one provider into `providers.<id>` — backup, merge into the existing
 * file, atomic write — and answers with the summary the file now holds.
 */
export function saveProvider(draft: ProviderDraft): Promise<ProviderSummary> {
  return invoke<ProviderSummary>("save_provider", { draft });
}

/**
 * S2: `GET {baseUrl}/models` from Rust, so the api key never reaches the webview
 * and CORS/CSP stay out of the way. An unreachable endpoint or a non-2xx answer
 * rejects with the message the form shows inline.
 */
export function discoverModels(
  baseUrl: string,
  apiKey: string,
): Promise<ModelOption[]> {
  return invoke<ModelOption[]>("discover_models", { baseUrl, apiKey });
}

// ── pi runtime seam (R3, R4) ─────────────────────────────────────────────────

export function startSession(workspaceId: number): Promise<SessionDto> {
  return invoke<SessionDto>("start_session", { workspaceId });
}

export function sendPrompt(
  workspaceId: number,
  sessionId: number,
  text: string,
): Promise<void> {
  return invoke<void>("send_prompt", { workspaceId, sessionId, text });
}

export function stopSession(
  workspaceId: number,
  sessionId: number,
): Promise<void> {
  return invoke<void>("stop_session", { workspaceId, sessionId });
}

/**
 * Deletes a session from the app: its pi process is stopped when it is live, its
 * transcript is moved to the archive folder under the app data dir, and the
 * index row is removed. `true` means a row was removed; an id that is not
 * indexed rejects instead of reporting a quiet success.
 */
export function deleteSession(sessionId: number): Promise<boolean> {
  return invoke<boolean>("delete_session", { sessionId });
}

export function sessionStatus(
  workspaceId: number,
  sessionId: number,
): Promise<SessionStatusDto> {
  return invoke<SessionStatusDto>("session_status", { workspaceId, sessionId });
}

/**
 * S3/S5 of `session-lifecycle.md`: makes sure the session has a process.
 *
 * Idempotent by contract: if pi is already live for this id the runtime returns
 * that session untouched, otherwise it spawns pi resuming the session's own
 * JSONL file. Selecting a session calls this; it is never a way to restart one.
 */
export function openSession(sessionId: string): Promise<SessionDto> {
  return invoke<SessionDto>("open_session", { sessionId });
}

/**
 * S3: the conversation a session already had, read by Rust from pi's JSONL file
 * (content never enters the app database). Oldest first, already capped by
 * Rust, and defensive about lines it does not model.
 */
export function sessionTranscript(
  sessionId: string,
): Promise<TranscriptMessage[]> {
  return invoke<TranscriptMessage[]>("session_transcript", { sessionId });
}

/**
 * L4: ids that currently have a live pi process, so the drawer can mark them
 * without claiming anything it cannot verify. An empty list means nothing is
 * running (S2: startup spawns nothing).
 */
export function liveSessionIds(): Promise<string[]> {
  return invoke<string[]>("live_session_ids");
}

/**
 * Subscribes to the event stream the Rust side forwards on `agent-event` (S6).
 * The listener lives here so no view imports the Tauri API directly (contract
 * seam rule, AGENTS.md 1 / S4). Resolves with an unsubscribe function; call it
 * when the view unmounts or it keeps receiving events after teardown.
 */
export async function subscribeAgentEvents(
  handler: (envelope: AgentEventEnvelope) => void,
): Promise<() => void> {
  return listen<AgentEventEnvelope>("agent-event", ({ payload }) => {
    handler(payload);
  });
}
