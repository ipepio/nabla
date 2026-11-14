// Contracts between Vue and Rust (spec S4).
// These interfaces mirror the Rust DTOs exactly (camelCase) and are the only
// channel between the Vue layer and Rust: Vue never talks to pi directly.

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
