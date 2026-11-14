// Typed wrappers around the Tauri commands (spec S4).
// Vue never talks to pi directly: every read and write goes through Rust.

import { invoke } from "@tauri-apps/api/core";
import type { AppSettings, SessionSummary, WorkspaceInfo } from "./types";

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

export function listSessions(workspaceId: number): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>("list_sessions", { workspaceId });
}

export function getSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_settings");
}

export function saveSettings(settings: AppSettings): Promise<AppSettings> {
  return invoke<AppSettings>("save_settings", { settings });
}
