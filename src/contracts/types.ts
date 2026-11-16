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
  /** S8: pins the space to the top of the list and drives the "Pinned" filter. */
  pinned: boolean;
  /**
   * S8: ISO-8601 timestamp of the last time the space was used, `null` until the
   * first touch. It is the recency source for the "Recent" filter.
   */
  lastUsedAt: string | null;
}

/**
 * One model of the flat `list_models` answer (S7): pi's own list is a Rust
 * constant. Since S2 of `odd/tasks/models-by-provider.md` the composer no longer
 * reads it — it groups `list_provider_models()` instead — but the command and its
 * DTO stay in the seam, so this mirror stays next to them.
 */
export interface ModelInfo {
  id: string;
  name: string;
}

/**
 * One provider pi knows, as `list_providers` summarizes it (S1 of
 * `odd/tasks/providers-pane.md`). The api key never crosses the seam: the DTO
 * carries only `hasApiKey` (S4), so the webview cannot read or leak it.
 * `set_provider_key` answers with this same shape after writing a key, so the
 * pane refreshes one row from what the file holds instead of guessing.
 * Mirrors `ProviderSummary` in `src-tauri/src/providers.rs`.
 */
export interface ProviderSummary {
  id: string;
  name: string;
  baseUrl: string;
  api: string;
  modelCount: number;
  hasApiKey: boolean;
}

/**
 * One model a provider offers, as `discover_models` reads it from
 * `GET {baseUrl}/models` (S2). Mirrors `ModelOption` in
 * `src-tauri/src/providers.rs`.
 */
export interface ModelOption {
  id: string;
  name: string;
}

/**
 * What `save_provider` writes into pi's `providers.<id>` (S3 of
 * `providers-pane.md`). An empty `api` keeps the one the file already has (and
 * defaults to `openai-completions` when it has none); an empty `name` falls back
 * to the id, both on the Rust side. Mirrors `ProviderDraft` there.
 */
export interface ProviderDraft {
  id: string;
  name: string;
  baseUrl: string;
  api: string;
  apiKey: string;
  models: ModelOption[];
}

// ── models of one provider (S2, S3 of `odd/tasks/models-by-provider.md`) ─────

/**
 * `ProviderModel.source`: which of pi's two model files the entry came from.
 * `configured` is `models.json` — the file the human edits, so the settings editor
 * may offer to change it; `catalog` is pi's own cached catalogue
 * (`~/.pi/agent/models-store.json`), which pi rewrites on its own schedule and
 * Nabla never writes.
 */
export type ProviderModelSource = "configured" | "catalog";

/**
 * One model of one provider, as `list_provider_models` reports it (S2/S3). Pi's
 * own field names (`contextWindow`, `maxTokens`, `cost.input`, `cost.output`,
 * `input`) are already flattened by Rust, so this is the shape the grouped
 * selector and the per-model editor read.
 *
 * `enabled` is Nabla's mark, not pi's (L3): a disabled model stays in
 * `models.json` with `"enabled": false` instead of being deleted, and the
 * selector hides it while the editor keeps showing it. Rust defaults a missing
 * mark to `true`.
 *
 * `source` says which of pi's two files the entry came from: `configured` is
 * `models.json`, `catalog` is pi's own cached catalogue
 * (`~/.pi/agent/models-store.json`), which pi refreshes on sign-in and owns
 * outright. A catalog entry is therefore READ-ONLY for Nabla: the editor shows
 * it and never writes it.
 *
 * Mirrors `ModelInfo` in `src-tauri/src/providers.rs`. The TS name is
 * `ProviderModel` because the flat `list_models` DTO above already owns
 * `ModelInfo`; every field matches the Rust struct field by field.
 */
export interface ProviderModel {
  id: string;
  name: string;
  contextWindow: number | null;
  maxTokens: number | null;
  inputCost: number | null;
  outputCost: number | null;
  /** Input modalities, pi's `input` array: `"text"` and `"image"`. */
  modalities: string[];
  /** L3: the model is offered by the selector and usable by a session. */
  enabled: boolean;
  /**
   * Nabla's own "do not offer it" mark: stored on the Nabla side, never in pi's
   * files. It is a display preference, so it is independent of `enabled` (pi's
   * usability mark) and it applies to a `catalog` row too, because hiding one
   * writes nothing pi owns. A hidden model is still usable — the session that
   * already chose it keeps it, and the editor keeps listing it — but the
   * composer's picker does not offer it. Rust defaults a missing mark to
   * `false`, so a payload from a build that predates it reads as visible.
   */
  hidden: boolean;
  /** Which of pi's files the entry came from; `catalog` is read-only. */
  source: ProviderModelSource;
}

/**
 * One provider with the models pi would use, in the order Rust lists them: the
 * `models.json` entries first, then the catalogue entries whose id the file does
 * not already carry (S2 of `models-by-provider.md`). A provider only pi's
 * catalogue knows — a freshly signed-in `openai` — is included too, so the
 * selector and the editor can show it. A provider with no models comes back with
 * an empty list, never filtered out; the api key has no field here (S4).
 * Mirrors `ProviderModels` in `src-tauri/src/providers.rs`.
 */
export interface ProviderModels {
  provider: string;
  name: string;
  models: ProviderModel[];
}

/**
 * What `update_provider_model` writes for one model inside its provider (S3).
 *
 * A `null` number, an empty `name` and an empty `modalities` list mean "the form
 * does not cover this field": Rust merges the draft into the entry the file
 * already has and keeps what is there, so an edit of the cost never wipes a
 * context window. `enabled` is always written. Mirrors `ProviderModelDraft` in
 * `src-tauri/src/providers.rs`.
 */
export interface ProviderModelDraft {
  provider: string;
  id: string;
  name: string;
  contextWindow: number | null;
  maxTokens: number | null;
  inputCost: number | null;
  outputCost: number | null;
  /** `"text"` and/or `"image"`; the two the editor offers. */
  modalities: string[];
  enabled: boolean;
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
  /** S7: pi provider id the spawn passes as `--provider`. */
  provider: string;
  /**
   * S7: model id the spawn passes as `--model`. S2 of
   * `models-by-provider.md`: it belongs to `provider` above, because both halves
   * are chosen together in the grouped selector.
   */
  model: string;
  /**
   * S2 of `odd/tasks/i18n-and-settings-polish.md`: interface language, mirrored
   * from Rust (`Settings.language`). The value the UI writes is always one of
   * `SUPPORTED_LOCALES` in `src/lib/i18n.ts`; Rust defaults a missing field to
   * `"en"`, and the UI normalizes anything else to English as well.
   */
  language: string;
  /**
   * S2 of `odd/tasks/chat-preferences-and-queue.md`: the "Show reasoning"
   * toggle. Off, the chat renders no reasoning block at all — neither the turn
   * being streamed nor a message read back from pi's file. Rust defaults the
   * field to `true` (`serde(default = ...)`), so a settings file written before
   * this preference existed keeps showing the reasoning.
   */
  showReasoning: boolean;
  /**
   * S3 of the same task: the "Queue messages" toggle. On, a prompt sent while
   * the agent is working is queued for that session instead of being blocked,
   * and the queue hands one prompt to the session each time a turn ends. Rust
   * defaults the field to `true`, so a settings file written before this
   * preference existed behaves like a fresh install.
   */
  queueMessages: boolean;
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
 * One turn of a pi-owned transcript, as `session_transcript` reads it back from
 * the JSONL file (S3 of `odd/tasks/session-lifecycle.md`). Mirrors
 * `TranscriptMessage` in `src-tauri/src/commands.rs`.
 *
 * `at` is pi's own timestamp string, `null` when the line carries none: the
 * store passes it through untouched and the view formats it for display.
 */
export interface TranscriptMessage {
  role: TranscriptRole;
  text: string;
  at: string | null;
}

/** The roles a pi transcript line can have; anything else is ignored by Rust. */
export type TranscriptRole = "user" | "assistant" | "tool";

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

// ── pi CLI bridge (S1-S4 of `odd/tasks/pi-cli-bridge.md`) ────────────────────

/**
 * Account state of one provider, exactly the words `pi auth check --json` can
 * report (S4). `unknown` is what Rust answers when the JSON cannot be
 * understood, so the UI never invents a login state it did not observe.
 * Mirrors `AuthState` in `src-tauri/src/cli.rs` (snake_case, as serde writes it).
 */
export type PiAuthState = "ready" | "not_ready" | "invalid" | "unknown";

/**
 * Typed account state of one provider (S4). `credential` says whether a
 * credential is present, never its value: pi keeps the secret in `auth.json`
 * and it never crosses this seam. Mirrors `PiAuthStatus` in
 * `src-tauri/src/cli.rs`.
 */
export interface PiAuthStatus {
  provider: string;
  state: PiAuthState;
  credential: boolean;
}

/**
 * One row of the persisted account cache (S5 of
 * `providers-pane-refinements.md`). It is the same observation as
 * `PiAuthStatus` plus the instant it was made, because a badge read from
 * `nabla.db` has to say how old it is. `checkedAt` is the ISO-8601 stamp Rust
 * keeps in `provider_status.checked_at`. Mirrors the Rust `ProviderStatusRow`
 * payload of the `provider_status` table.
 */
export interface ProviderStatusRow {
  provider: string;
  state: PiAuthState;
  credential: boolean;
  /** ISO-8601 instant of the last check that produced this row. */
  checkedAt: string;
}

/**
 * Answer of one non-interactive `pi` call (S1, S2). A non-zero `exitCode` is an
 * answer, not an error; `timedOut` says the deadline killed the child and
 * `truncated` says a stream hit Rust's 64 KiB cap. Mirrors `PiCliResult` in
 * `src-tauri/src/cli.rs`.
 */
export interface PiCliResult {
  stdout: string;
  stderr: string;
  /** `null` when the child was killed by a signal, so no code exists. */
  exitCode: number | null;
  timedOut: boolean;
  truncated: boolean;
}

/**
 * The three things one interactive session reports (S3): one line of stdout,
 * one line of stderr, or the exit of the child.
 */
export type PiCliStream = "stdout" | "stderr" | "exit";

/**
 * Payload of the Tauri event `pi-cli` (S3). `line` carries one complete line
 * without its newline and is absent on `exit`; `exitCode` is present only on
 * `exit`, and only when the child left on its own. Mirrors `CliEvent` in
 * `src-tauri/src/cli.rs`.
 */
export interface PiCliEvent {
  cliSessionId: string;
  stream: PiCliStream;
  line?: string;
  exitCode?: number;
}
