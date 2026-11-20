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
  /**
   * v5 of `odd/tasks/subagent-helpers.md`: the chat that delegated to this
   * session, or `null` for a session the human started. It is what nests a child
   * under its parent in the sidebar instead of listing it as another chat.
   */
  parentSessionId: number | null;
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
  /**
   * S5 of `odd/tasks/tool-drawer-and-mcp.md`: which drawer tools are offered,
   * by id (`files`, `changes`, `terminal`, `browser`). Rust defaults the field to
   * `["files", "changes"]`, so the two read-only tools are on and the two that
   * run code are opt-in. An id this build does not know is simply never matched
   * by the registry, so enabling a future tool early changes nothing.
   */
  enabledTools: string[];
  /**
   * S1/S6 of `odd/tasks/permission-modes.md`: the mode the agent obeys, mirrored
   * from `Settings.permission_mode`. Rust defaults it to `"write"` and reads an
   * unknown value as `"write"` too, so nothing here may assume `"yolo"`.
   */
  permissionMode: PermissionMode;
  /**
   * S2 of `odd/tasks/composer-inputs.md`: the thinking level (effort) a NEW
   * session starts with, mirrored from `Settings.thinking_level`. Rust defaults
   * it to `"medium"` — pi's own documented `defaultThinkingLevel` — and keeps an
   * unknown word verbatim while reading it as "no level", so a value here is not
   * guaranteed to be one of the levels this build knows.
   */
  thinkingLevel: ThinkingLevel;
  /**
   * S1 of `odd/tasks/web-search.md`: the agent's `web_search` tool. Rust defaults
   * the field to `false`, so a settings file written before this preference
   * existed — and a fresh install — searches nothing until the human turns it on.
   */
  webSearchEnabled: boolean;
  /**
   * S1: the SearXNG instance that answers `web_search`, as typed. Empty means
   * "try any public instance", which is best effort: public instances rate-limit
   * or refuse the JSON API.
   */
  webSearchInstance: string;
}

/**
 * S1 of `odd/tasks/permission-modes.md`: the three modes, exactly as the file
 * and the pi extension spell them.
 *
 *   `read`  only inspects: `bash`, `powershell`, `edit` and `write` are blocked
 *   `write` edits files, and ASKS before running a command
 *   `yolo`  asks nothing at all
 *
 * Mirrors `PermissionMode` in `src-tauri/src/settings.rs`.
 */
export type PermissionMode = "read" | "write" | "yolo";

/**
 * S2 of `odd/tasks/composer-inputs.md`: pi's thinking levels (effort). Mirrors
 * `THINKING_LEVELS` in `src-tauri/src/settings.rs`, which is also pi's own list
 * for `--thinking` (pi docs/cli.md:71).
 *
 * The union is widened instead of closed: pi clamps the level to what the model
 * accepts and reports the result through `thinking_level_changed`, so a word a
 * newer pi introduces must still cross this seam.
 */
export type ThinkingLevel =
  | "off"
  | "minimal"
  | "low"
  | "medium"
  | "high"
  | "xhigh"
  | "max"
  | (string & {});

/**
 * S3 of `composer-inputs.md`: one command the session offers, as pi reports it in
 * its `get_commands` answer. Mirrors `SessionCommand` in
 * `src-tauri/src/commands.rs`, which keeps only what the list renders.
 *
 * `name` never carries the leading slash: the composer adds it back when it
 * completes the text, and sending `/name` is what invokes the command (pi expands
 * it, Nabla never interprets one). `description` is absent when pi sent none, and
 * `source` is pi's own word — `extension`, `prompt` or `skill` — kept verbatim so
 * the badge cannot lie about where a command comes from.
 */
export interface SessionCommand {
  name: string;
  description?: string;
  source: string;
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
 * One child a chat delegated to, as `subagent_sessions` answers it (H3 of
 * `odd/tasks/subagent-helpers.md`). Mirrors `SubagentSession` in
 * `src-tauri/src/commands.rs`.
 *
 * `status` is read from the extension's manifest on every call, never from the
 * database, so it cannot go stale: `queued`, `running`, `done`, `failed`, or
 * `unknown` when the manifest no longer names a child the index still has. The
 * child's transcript is a normal pi session file — `sessionTranscript` takes
 * `sessionId` and works unchanged, which is why there is no child-specific reader.
 */
export interface SubagentSession {
  sessionId: number;
  /** The name of this run, what the parent's `agent_*` tools address. */
  name: string;
  /** The agent definition it runs. */
  agent: string;
  /** Absolute path of the child's own pi JSONL file. */
  sessionPath: string;
  status: SubagentStatus;
  startedAt: string | null;
  endedAt: string | null;
}

/** The states the manifest can report, plus the honest "we cannot tell". */
export type SubagentStatus = "queued" | "running" | "done" | "failed" | "unknown";

/**
 * One subagent profile, as the human configures it in Settings → Subagents (S1,
 * S2 of `odd/tasks/agent-profiles.md`). Mirrors `AgentProfile` in
 * `src-tauri/src/agents.rs` field by field.
 *
 * The store is Nabla's own `<data dir>/agents.json`, never a file inside the
 * project. `tools` is the child's allowlist as configured — an empty list is a
 * real answer ("no tools"), not "the defaults". `model` is `provider/model` or a
 * bare model id, resolved against the session's authenticated registry at run
 * time; `effort` is one of pi's thinking levels; `instructions` is the child's
 * whole system prompt.
 */
export interface AgentProfile {
  name: string;
  description: string;
  tools: string[];
  model: string | null;
  effort: string | null;
  instructions: string;
  /** S3: a profile ships off, and only enabled profiles are offered. */
  enabled: boolean;
}

/**
 * The whole profile store (S1, S3, S6 of `agent-profiles.md`). Mirrors
 * `AgentsConfig` in `src-tauri/src/agents.rs`.
 *
 * `enabled` is the master switch: off, the subagent extension is never loaded,
 * so `run_agent` does not exist for the model. `modelDefault` is the second level
 * of the model resolution order, between a profile's own model and the parent
 * session's. A missing file is this shape with everything off — which is what a
 * fresh install reads as.
 */
export interface AgentsConfig {
  enabled: boolean;
  modelDefault: string | null;
  profiles: AgentProfile[];
}

/**
 * One message of the preview the app keeps for a session: the newest turns as it
 * last streamed them, so opening a session paints before pi boots. Mirrors
 * `PreviewMessage` in `src-tauri/src/commands.rs`.
 *
 * `reasoning` is the part that makes this preview worth keeping: pi's own JSONL
 * file never stored it, so the file read alone cannot reproduce it. `reasoning`
 * and `at` are `null` when the turn carried none.
 */
export interface PreviewMessage {
  role: TranscriptRole;
  text: string;
  reasoning: string | null;
  at: string | null;
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

/**
 * The model one live session is really running, as pi answered `set_model` (S2 of
 * `odd/tasks/models-by-provider.md`): its own provider, id and name, so the
 * composer can trust the engine instead of its own echo. Mirrors
 * `SessionModelInfo` in `src-tauri/src/commands.rs`.
 */
export interface SessionModelInfo {
  /** Provider the model belongs to: model ids are provider-scoped. */
  provider: string;
  id: string;
  /** Human label; pi always sends one and Rust falls back to the id. */
  name: string;
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

// ── MCP servers (F0 of `odd/tasks/tool-drawer-and-mcp.md`) ───────────────────

/**
 * Which `mcp.json` file defines a server: `global` is
 * `~/.pi/agent/mcp.json` (shared by every workspace) and `project` is
 * `.pi/mcp.json` in the workspace (read only in a trusted project). Mirrors
 * `McpScope` in `src-tauri/src/mcp.rs`.
 */
export type McpScope = "global" | "project";

/**
 * How pi reaches a server. `stdio` spawns a process (`command` + `args`);
 * `http` connects to a streamable HTTP endpoint (`url`). The legacy SSE
 * transport pi rejects is deliberately not representable here.
 */
export type McpTransport = "stdio" | "http";

/**
 * The state of one server as `pi mcp list --json` reports it, reduced to the
 * five words the pane can paint (S1):
 *
 * - `ready` — connected, so its tools are usable;
 * - `needs_sign_in` — the server rejected the connection and wants OAuth;
 * - `error` — the connection failed, and `error` carries pi's reason;
 * - `disabled` — the entry is present with `enabled: false`, or Rust could not
 *   connect it because the human turned it off;
 * - `unknown` — pi said something this build does not model (a newer state
 *   word), so the badge never claims a state nobody observed.
 *
 * Mirrors `McpState` in `src-tauri/src/mcp.rs` (snake_case, as serde writes it).
 */
export type McpState =
  | "ready"
  | "needs_sign_in"
  | "error"
  | "disabled"
  | "unknown";

/**
 * One server of `mcp_list()` (S1). It is a read-only summary of what pi's
 * configuration plus one connection test answered: `target` is the command line
 * of a stdio server or the URL of an HTTP one, and `tools` are the tool names
 * the server published (`[]` when it never connected). No credential has a
 * field here: OAuth tokens stay in pi's `mcp-auth.json`, and `headers`/`env`
 * values are never read back. Mirrors `McpServerSummary` in
 * `src-tauri/src/mcp.rs`.
 */
export interface McpServerSummary {
  name: string;
  scope: McpScope;
  transport: McpTransport;
  /** `command args` for stdio, the URL for http; never a secret. */
  target: string;
  /** `false` when the entry exists with `enabled: false`. */
  enabled: boolean;
  state: McpState;
  /** Tool names the server published; empty when it never connected. */
  tools: string[];
  /** pi's own reason when the server did not connect, `null` otherwise. */
  error: string | null;
}

/**
 * One `key=value` pair of the add form: an environment variable of a stdio
 * server or an HTTP header. Both are written as pi's `env`/`headers` objects,
 * so a value may be `${NAME}` or `!command` exactly as `mcp.json` allows.
 */
export interface McpKeyValue {
  key: string;
  value: string;
}

/**
 * What `mcp_add` writes into one `mcp.json` (S1). The two halves are exclusive:
 * a stdio draft carries `command` + `args` + `env`, an http draft carries `url` +
 * `headers` + an optional `bearerTokenEnvVar` (pi sends
 * `Authorization: Bearer ${NAME}`). Rust validates the shape and writes the file;
 * the caller never builds the argv itself. Mirrors `McpAddDraft` in
 * `src-tauri/src/mcp.rs`.
 */
export interface McpAddDraft {
  name: string;
  scope: McpScope;
  transport: McpTransport;
  /** stdio only: one executable, never a shell string. */
  command?: string | null;
  /** stdio only: the arguments of `command`, in order. */
  args?: string[];
  /** stdio only: the process environment. */
  env?: McpKeyValue[];
  /** http only: the streamable HTTP endpoint. */
  url?: string | null;
  /** http only: headers sent with every request. */
  headers?: McpKeyValue[];
  /** http only: the variable pi reads for `Authorization: Bearer`. */
  bearerTokenEnvVar?: string | null;
  /**
   * How the model reaches the tools (`--exposure`): `direct` declares them as
   * `mcp__<server>__<tool>`; pi's default `codemode` declares nothing.
   */
  exposure?: string | null;
}

// ── shell sessions (F2 of `odd/tasks/tool-drawer-and-mcp.md`) ───────────────

/**
 * What `shell_open` returns: the id the other shell commands take, the pid of the
 * child the PTY owns, and the executable that was spawned (the human's login
 * shell, or `$SHELL`). Mirrors `ShellSession` in `src-tauri/src/shell.rs`.
 */
export interface ShellSession {
  sessionId: string;
  /**
   * Session leader of the PTY, when the platform reports one: `null` when it does
   * not. Informational only — the tab never needs it to drive the shell.
   */
  pid: number | null;
  /** Path or name of the shell the PTY runs; informational only. */
  shell: string;
}

/**
 * The two things a shell session reports: `data` is a chunk of the child's output
 * exactly as it came off the PTY (no line splitting — the emulator needs the raw
 * bytes to render carriage returns and colours), `exit` is the child leaving.
 */
export type ShellOutputStream = "data" | "exit";

/**
 * Payload of the Tauri event `shell-output` (F2). `data` is present only on
 * `data`, and `exitCode` only on `exit`, and only when the child left on its own
 * (a killed child has no code). Mirrors `ShellOutputEvent` in
 * `src-tauri/src/shell.rs`.
 */
export interface ShellOutputEvent {
  sessionId: string;
  stream: ShellOutputStream;
  data?: string;
  exitCode?: number;
}

// ── workspace files (F1 of `odd/tasks/tool-drawer-and-mcp.md`) ───────────────

/**
 * What one entry of `fs_list_dir` is. A symlink is its own kind on purpose: Rust
 * confines every read to the workspace, so the UI shows the link as a link and
 * never pretends it is the target it may point outside.
 */
export type FsEntryKind = "dir" | "file" | "symlink";

/**
 * One row of a directory listing: the name shown, the path the next call takes
 * back (workspace-relative, `/`-separated) and the size of a file in bytes.
 * Directories carry `0`. Mirrors `FsEntry` in `src-tauri/src/fs.rs`.
 */
export interface FsEntry {
  name: string;
  /** Workspace-relative path; hand it back to `fsListDir`/`fsReadPreview`. */
  path: string;
  kind: FsEntryKind;
  sizeBytes: number;
}

/**
 * How Rust classifies a preview before any content is sent.
 *
 * - `text` — decoded UTF-8 in `content`;
 * - `image` — a supported image; the bytes come from `fsReadImageBytes`;
 * - `binary` — no textual rendering, and no preview is attempted;
 * - `too_large` — above Rust's preview cap, so `content` stays `null` and the
 *   size is still reported.
 *
 * Mirrors `FsPreview` in `src-tauri/src/fs.rs`.
 */
export type FsPreviewKind = "text" | "image" | "binary" | "too_large";

/**
 * What `fs_read_preview` answers (S3 of `tool-drawer-and-mcp.md`): the kind, the
 * size of the file and, for `text` only, its content. `mime` is the image type
 * when Rust recognized one and `null` for everything else. There is no bytes
 * field on purpose: an image is fetched by `fsReadImageBytes`, which answers the
 * raw binary body instead of a base64 string.
 */
export interface FsPreview {
  kind: FsPreviewKind;
  sizeBytes: number;
  mime: string | null;
  /** UTF-8 text for `kind: "text"`; `null` for every other kind. */
  content: string | null;
}

// ── workspace changes (F1 of `odd/tasks/tool-drawer-and-mcp.md`) ────────────

/**
 * The change one path carries, reduced to the words the Changes tab paints:
 * a file staged for commit and then edited again appears as two entries, one
 * `staged: true` and one `staged: false`, exactly as `git status` reports it.
 * Mirrors `GitEntryStatus` in `src-tauri/src/git.rs`.
 */
export type GitEntryStatus =
  | "modified"
  | "added"
  | "deleted"
  | "renamed"
  | "untracked"
  | "conflicted";

/**
 * One row of `git_status`: the workspace-relative path, its change and whether
 * the change lives in the index (`staged`) or in the working tree.
 * Mirrors `GitEntry` in `src-tauri/src/git.rs`.
 */
export interface GitEntry {
  path: string;
  status: GitEntryStatus;
  staged: boolean;
}

// ── the question bridge (B3 of `odd/tasks/agent-question-bridge.md`) ─────────

/**
 * The answer the human gave to one `extension_ui_request`, exactly as
 * `answer_ui_request` takes it. Mirrors `UiAnswer` in `src-tauri/src/rpc/mod.rs`:
 * an untagged union of the three shapes the protocol defines — a chosen or typed
 * value, a confirmation, or a cancellation.
 *
 * `cancelled` is the literal `true` on purpose: the protocol only defines a
 * cancellation, so there is no "uncancelled" answer to send.
 */
export type UiAnswer =
  | { value: string }
  | { confirmed: boolean }
  | { cancelled: true };

// ── embedded browser (child webview of the main window) ─────────────────────

/**
 * One `embed-nav` event: the view that navigated and the url it is now showing.
 * Mirrors `EmbedNavEvent` in `src-tauri/src/embed.rs` field for field; the url is
 * the page the human is looking at, not the one the app last requested, which is
 * why the address bar follows this event instead of its own draft.
 */
export interface EmbedNavEvent {
  viewId: string;
  url: string;
}

// ── the app's own MCP endpoint (T3 of `odd/tasks/webview-control.md`) ───────

/**
 * The local MCP endpoint this run of the app offers so the agent can drive the
 * embedded views, and the token that opens it (S6). Nothing renders it yet: the
 * app writes the registration into pi's `mcp.json` itself, so this mirror exists
 * to keep the seam symmetric — the command, its DTO and this type move together.
 *
 * The token is a secret of this run, not of the human's account: it dies with the
 * process, and it never leaves the machine. Mirrors `Endpoint` in
 * `src-tauri/src/mcp_server.rs`.
 */
export interface McpEndpoint {
  /** `http://127.0.0.1:<port>/mcp` — an ephemeral port of this run. */
  url: string;
  /** The bearer token every request to that endpoint must carry. */
  token: string;
}
