// Contracts between Vue and Rust (spec S4).
// These interfaces mirror the Rust DTOs exactly (camelCase) and are the only
// channel between the Vue layer and Rust: Vue never talks to pi directly.

import type { RuntimeStatus } from "./agent-events";

/** A workspace owned by the app store (SQLite). */
export interface WorkspaceInfo {
  id: number;
  path: string;
  name: string;
  createdAt: string;
}

/** Index entry for a pi-owned session file. Content is never copied here. */
export interface SessionSummary {
  id: number;
  workspaceId: number;
  piSessionPath: string;
  title: string | null;
  model: string | null;
  startedAt: string;
  updatedAt: string;
}

/** User-editable preferences persisted in settings.json. */
export interface AppSettings {
  theme: string;
  piBinary: string;
}

/**
 * What `start_session` returns: the registered session, the pi session file it
 * owns, and the live process. Mirrors `SessionDto` in
 * `src-tauri/src/commands.rs`; `sessionId` is the index row id in decimal form,
 * the key every later session command takes.
 */
export interface SessionDto {
  sessionId: string;
  workspaceId: number;
  piSessionPath: string;
  pid: number;
  status: RuntimeStatus;
}

/**
 * What `session_status` returns. A session that is not running reports
 * `stopped` with no pid. Mirrors `SessionStatusDto` in
 * `src-tauri/src/commands.rs`.
 */
export interface SessionStatusDto {
  sessionId: string;
  status: RuntimeStatus;
  pid: number | null;
  error: string | null;
}
