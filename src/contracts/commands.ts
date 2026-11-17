// Typed wrappers around the Tauri commands (spec S4).
// Vue never talks to pi directly: every read and write goes through Rust.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import type { AgentEventEnvelope } from "./agent-events";
import type {
  AppSettings,
  BrowserCandidate,
  BrowserSession,
  BrowserTarget,
  FsEntry,
  FsPreview,
  GitEntry,
  McpAddDraft,
  McpScope,
  McpServerSummary,
  ModelInfo,
  ModelOption,
  PiAuthStatus,
  PiCliEvent,
  PiCliResult,
  ProviderDraft,
  ProviderModel,
  ProviderModelDraft,
  ProviderModels,
  ProviderStatusRow,
  ProviderSummary,
  SessionDto,
  SessionStatusDto,
  SessionSummary,
  ShellOutputEvent,
  ShellSession,
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

/**
 * S3: removes `providers.<id>` from pi's configuration file — backup and atomic
 * write included — and answers with the summary the entry now has.
 *
 * `true` means the entry was removed. `false` means the file no longer had it:
 * the delete is idempotent, so a stale row cannot turn into an error. A
 * malformed id still rejects before anything is written.
 */
export function deleteProvider(providerId: string): Promise<boolean> {
  return invoke<boolean>("delete_provider", { providerId });
}

/**
 * S4: stores the api key of an existing provider in pi's plain-text file and
 * answers with the summary `list_providers` would report, so the pane refreshes
 * from it instead of guessing. The key itself never comes back across the bridge.
 */
export function setProviderKey(
  providerId: string,
  apiKey: string,
): Promise<ProviderSummary> {
  return invoke<ProviderSummary>("set_provider_key", { providerId, apiKey });
}

// ── models of one provider (S2, S3 of `models-by-provider.md`) ──────────────

/**
 * S2: every provider pi's `models.json` lists, each with its own models in file
 * order, followed by the models of pi's cached catalogue that the file does not
 * carry yet (a provider only the catalogue knows — a freshly signed-in `openai`
 * — is included too). The grouped selector and the per-model editor are both
 * built on this one read; a provider with no models comes back with an empty
 * list instead of disappearing, and the api key never crosses (the DTO has no
 * field for it).
 *
 * Every entry carries `source`: only `configured` rows may be written back
 * through `update_provider_model` / `remove_provider_model`, because `catalog`
 * rows belong to pi's cache and pi rewrites them on its own schedule. The editor
 * renders those rows read-only for exactly that reason.
 */
export function listProviderModels(): Promise<ProviderModels[]> {
  return invoke<ProviderModels[]>("list_provider_models");
}

/**
 * S3: writes one model inside `providers.<provider>.models` — backup, merge into
 * the entry the file already has, atomic write — and answers with the entry as
 * the file now holds it, so the editor refreshes from what was written instead
 * of from what it sent.
 *
 * A `null` number, an empty `name` and an empty `modalities` list mean "keep what
 * the file has": Rust only writes the fields the draft covers. Disabling a model
 * is a mark (`enabled: false`), never a delete (L3).
 */
export function updateProviderModel(
  draft: ProviderModelDraft,
): Promise<ProviderModel> {
  return invoke<ProviderModel>("update_provider_model", { draft });
}

/**
 * S3: removes one model from one provider, backup and atomic write included.
 *
 * `true` means an entry was removed; `false` means the file no longer had it, so
 * the delete is idempotent and a stale row cannot turn into an error. A
 * malformed id still rejects before anything is written.
 */
export function removeProviderModel(
  provider: string,
  modelId: string,
): Promise<boolean> {
  return invoke<boolean>("remove_provider_model", { provider, modelId });
}

/**
 * Hides or shows one model in the composer's picker. `hidden` is Nabla's own mark
 * — stored on the Nabla side, not in pi's files — so the model stays exactly where
 * it is and a session already using it keeps it; only the picker stops offering
 * it. Because nothing of pi's is written, the mark is available for every row
 * `listProviderModels` returns, a read-only `catalog` row included.
 *
 * Answers whether the stored mark changed: the call is idempotent, so hiding an
 * already hidden model (or showing a visible one) writes nothing and answers
 * `false`. The caller therefore updates its row from the mark it asked for, not
 * from the answer. Mirrors `set_model_hidden` in `src-tauri/src/commands.rs`.
 */
export function setModelHidden(
  provider: string,
  modelId: string,
  hidden: boolean,
): Promise<boolean> {
  return invoke<boolean>("set_model_hidden", { provider, modelId, hidden });
}

/**
 * S5 of `providers-pane-refinements.md`: the models one CONFIGURED provider
 * offers, read by Rust from `GET {baseUrl}/models` with the base URL and api key
 * of that provider in pi's `models.json`. Resolving both server-side is what
 * keeps the key out of the webview and CORS/CSP out of the way.
 *
 * Rejects when the provider is unknown to the file or has no api key yet, with
 * the reason the editor shows inline. The answer is the endpoint's raw list: the
 * caller decides which entries are new, so "add" never overwrites a model that is
 * already configured.
 */
export function discoverProviderModels(
  providerId: string,
): Promise<ModelOption[]> {
  return invoke<ModelOption[]>("discover_provider_models", { providerId });
}

// ── pi CLI bridge seam (S1-S4 of `odd/tasks/pi-cli-bridge.md`) ───────────────

/**
 * S4: the account state of one provider, read by Rust from
 * `pi auth check --provider <p> --json`. The credential itself stays in pi's
 * `auth.json`: only presence and state cross the seam.
 */
export function piAuthCheck(provider: string): Promise<PiAuthStatus> {
  return invoke<PiAuthStatus>("pi_auth_check", { provider });
}

/**
 * S2 of `providers-pane-refinements.md`: the account state of many providers in
 * one call. One `pi auth check` boots a whole CLI (~1.15 s), so the pane asks
 * once instead of once per row: Rust probes them through a bounded pool, keeps
 * the request order, and answers `unknown` for a provider that times out or
 * cannot be spawned instead of failing the batch.
 *
 * The answer has one entry per requested provider, so the caller maps by
 * `provider` and never by position.
 */
export function piAuthCheckMany(providers: string[]): Promise<PiAuthStatus[]> {
  return invoke<PiAuthStatus[]>("pi_auth_check_many", { providers });
}

/**
 * S5 of `providers-pane-refinements.md`: the last observation of every provider,
 * read straight out of `nabla.db`. It boots no CLI — it is a plain table read,
 * milliseconds — so the pane uses it to paint the badges of a previous run
 * before any probe is scheduled, and every row carries the instant of the check
 * that produced it. An empty table is an empty answer, not an error.
 *
 * The batched probe above is what writes it: `pi_auth_check_many` persists each
 * result on its way out, so the cache is always a copy of an answer pi already
 * gave, never a value this side invented.
 */
export function cachedProviderStatuses(): Promise<ProviderStatusRow[]> {
  return invoke<ProviderStatusRow[]>("cached_provider_statuses");
}

/**
 * S1/S2: runs one allowlisted `pi` invocation and captures its output. The
 * allowlist (`auth`, `models`, `config`, `--version`, `--help`), the argv-only
 * spawn and the bounded output live in Rust; `timeoutMs` is optional and Rust
 * defaults it to 20 s.
 *
 * A non-zero exit is an answer, not a rejection: `exitCode` carries it. Only a
 * call that could not run at all rejects.
 */
export function piCliRun(
  args: string[],
  timeoutMs?: number,
): Promise<PiCliResult> {
  return invoke<PiCliResult>("pi_cli_run", {
    args,
    timeoutMs: timeoutMs ?? null,
  });
}

/**
 * S3: starts an interactive invocation — the OAuth device-code login — and
 * answers with the session id its `pi-cli` events carry. There is no timeout:
 * the human decides when the flow ends, and `piCliStop` is how it ends.
 */
export function piCliStart(args: string[]): Promise<string> {
  return invoke<string>("pi_cli_start", { args });
}

/**
 * S3: pastes text into the live child's stdin (Rust appends the newline), so the
 * returned device code reaches `pi auth login`. `false` means the session is
 * gone or already finished, which is not an error.
 */
export function piCliWrite(
  cliSessionId: string,
  text: string,
): Promise<boolean> {
  return invoke<boolean>("pi_cli_write", { cliSessionId, text });
}

/**
 * S3: kills the live child and forgets the session. Idempotent: `false` means
 * there was nothing left to stop.
 */
export function piCliStop(cliSessionId: string): Promise<boolean> {
  return invoke<boolean>("pi_cli_stop", { cliSessionId });
}

/**
 * Subscribes to the `pi-cli` event stream one interactive session produces (S3).
 * The listener lives here so no component imports the Tauri API directly
 * (contract-seam rule, AGENTS.md 1). Resolves with an unsubscribe function: call
 * it on unmount or it keeps receiving events after teardown.
 */
export async function subscribePiCli(
  handler: (event: PiCliEvent) => void,
): Promise<() => void> {
  return listen<PiCliEvent>("pi-cli", ({ payload }) => {
    handler(payload);
  });
}

// ── system opener seam (S3 of `odd/tasks/pi-cli-bridge.md`) ──────────────────

/**
 * S3: hands an absolute `http(s)` URL to the system browser through the opener
 * plugin the app already installs (`opener:default` capability). The import
 * stays here because no component imports the Tauri API directly (contract-seam
 * rule, AGENTS.md 1).
 *
 * Only URLs the UI just generated are ever passed through: nothing is stored, so
 * there is no history to replay. A rejected call means the browser did not open;
 * the caller keeps the URL on screen as the manual fallback.
 */
export function openExternalUrl(url: string): Promise<void> {
  // Only web links: the opener can also reach local files and handlers, and this
  // seam exists for the sign-in URL a browser has to load.
  if (!/^https?:\/\//i.test(url)) {
    return Promise.reject(
      new Error("Only absolute http(s) URLs can be opened."),
    );
  }
  return openUrl(url);
}

/**
 * S1 of `tool-drawer-and-mcp.md`: shows one absolute path in the system file
 * manager, with the file already selected.
 *
 * Why revealing and not "open with the default app": the opener plugin grants
 * `open-path` only to a capability that lists `opener:allow-open-path`, and this
 * app's capability (`opener:default`) does not, so a webview call to it would be
 * denied at runtime. `reveal_item_in_dir` IS part of `opener:default`, so this is
 * the file action that works today without widening the Tauri capability (a
 * `src-tauri/` change, outside this task). The import stays here because no
 * component imports the Tauri API directly (contract-seam rule, AGENTS.md 1).
 *
 * Only an absolute path is accepted: the value comes from `mcpFilePath`, and a
 * relative or empty one would be revealed somewhere nobody asked for.
 */
export function revealFileInFileManager(path: string): Promise<void> {
  const absolute = /^\//.test(path) || /^[A-Za-z]:[\\/]/.test(path);
  if (path.trim() === "" || !absolute) {
    return Promise.reject(new Error("Only absolute paths can be revealed."));
  }
  return revealItemInDir(path);
}

// ── MCP servers (F0 of `odd/tasks/tool-drawer-and-mcp.md`) ───────────────────

/**
 * S1: every server pi's MCP configuration defines, global and project, as pi
 * itself reports it. Rust runs `pi mcp list --json` and maps its answer; a server
 * that cannot connect is a row with `state: "error"` and pi's reason, never a
 * rejection, so one broken entry never hides the others.
 *
 * It is a full connect-and-probe of every enabled server, so the pane treats it
 * as background work with a visible "refreshing" face instead of a synchronous
 * read.
 */
export function mcpList(): Promise<McpServerSummary[]> {
  return invoke<McpServerSummary[]>("mcp_list");
}

/**
 * S1: writes one server into the `mcp.json` of its scope through
 * `pi mcp add … -- <command> [args]` / `--url <url>` (env, headers and
 * `--bearer-token-env-var` included) and answers with the refreshed list, so the
 * pane renders what the file now holds instead of patching its own copy of a row
 * it did not write. A rejected draft reports Rust's reason (an invalid name, an
 * unparsable URL), and the form keeps what was typed.
 */
export function mcpAdd(draft: McpAddDraft): Promise<McpServerSummary[]> {
  return invoke<McpServerSummary[]>("mcp_add", { draft });
}

/**
 * S1: removes one server from the file of its scope (`pi mcp remove`, `-l` for a
 * project entry) and answers with the refreshed list. pi keeps the stored OAuth
 * credentials of a removed entry, so this never signs anything out.
 */
export function mcpRemove(
  name: string,
  scope: McpScope,
): Promise<McpServerSummary[]> {
  return invoke<McpServerSummary[]>("mcp_remove", { name, scope });
}

/**
 * S1: starts the OAuth sign-in of one server (`pi mcp login <name>`, `-l` for a
 * project entry) and answers with the id of its interactive session. There is no
 * timeout: the human decides when the flow ends, and `mcpLogout`, `piCliStop` or
 * the child's own exit is how it ends.
 *
 * The child's output arrives on the existing `pi-cli` event (subscribe with
 * `subscribePiCli`), and the pasted redirect URL goes back through
 * `piCliWrite` — the same path the provider device-code login already uses, so
 * there is one interactive-CLI seam and not two.
 */
export function mcpLoginStart(
  name: string,
  scope: McpScope,
): Promise<string> {
  return invoke<string>("mcp_login_start", { name, scope });
}

/**
 * S1: deletes the OAuth credentials pi stored for one server (`pi mcp logout`) and
 * answers with the refreshed list, so the badge turns `needs_sign_in` because pi
 * said so. The entry itself stays in `mcp.json`.
 */
export function mcpLogout(
  name: string,
  scope: McpScope,
): Promise<McpServerSummary[]> {
  return invoke<McpServerSummary[]>("mcp_logout", { name, scope });
}

/**
 * S1: the absolute path of the `mcp.json` of one scope, resolved by Rust so the
 * dialog never guesses a directory (`~/.pi/agent/mcp.json` or the workspace's
 * `.pi/mcp.json`). Hand it to `revealFileInFileManager` for the "show file"
 * action; nothing of the file's content crosses the seam.
 */
export function mcpFilePath(scope: McpScope): Promise<string> {
  return invoke<string>("mcp_file_path", { scope });
}

// ── shell sessions (F2 of `odd/tasks/tool-drawer-and-mcp.md`) ───────────────

/**
 * F2/S5: starts a real shell inside one workspace. Rust spawns it on a PTY with
 * `cwd = workspace` and answers the session id every other shell call takes; the
 * child's output arrives on the `shell-output` event, never in this answer.
 *
 * `workspace` is the absolute path the drawer received from the store, so the
 * shell opens where the active workspace is, with no directory guessing here.
 */
export function createShell(payload: {
  workspace: string;
}): Promise<ShellSession> {
  return invoke<ShellSession>("shell_open", { workspace: payload.workspace });
}

/**
 * F2: writes raw keystrokes to the shell's stdin. `data` is exactly what the
 * emulator produced (a printable run, a control byte, a pasted block), so no
 * newline is appended and nothing is escaped. `false` means the session is gone
 * or already finished, which is not an error.
 */
export function writeShell(sessionId: string, data: string): Promise<boolean> {
  return invoke<boolean>("shell_write", { sessionId, data });
}

/**
 * F2: tells the PTY its new size, in character cells, so full-screen programs
 * (`vim`, `top`) draw at the size the drawer is showing. `false` means the
 * session is gone, which is not an error.
 */
export function resizeShell(
  sessionId: string,
  cols: number,
  rows: number,
): Promise<boolean> {
  return invoke<boolean>("shell_resize", { sessionId, cols, rows });
}

/**
 * F2: kills the shell and forgets the session. Idempotent: `false` means there
 * was nothing left to close, so teardown never has to guess whether it already
 * ran.
 */
export function closeShell(sessionId: string): Promise<boolean> {
  return invoke<boolean>("shell_close", { sessionId });
}

/**
 * F2: subscribes to the `shell-output` stream. One listener serves every session,
 * so the caller filters by its own `sessionId`; the listener lives here because
 * no component imports the Tauri API directly (contract-seam rule, AGENTS.md 1).
 * Resolves with an unsubscribe function: call it on unmount or it keeps receiving
 * events after teardown.
 */
export async function subscribeShellOutput(
  handler: (event: ShellOutputEvent) => void,
): Promise<() => void> {
  return listen<ShellOutputEvent>("shell-output", ({ payload }) => {
    handler(payload);
  });
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

// ── workspace files and changes (F1 of `odd/tasks/tool-drawer-and-mcp.md`) ──

/**
 * S3: one directory of the workspace, one level deep. `relPath` is relative to
 * `workspace` and `""` is the workspace root; the answer never includes parent
 * entries, so the Files tab can expand lazily — one call per expanded folder,
 * never a recursive walk. Rust confines every read to the workspace, so a `..`
 * or a symlink that escapes is a rejection, not an answer.
 */
export function fsListDir(
  workspace: string,
  relPath: string,
): Promise<FsEntry[]> {
  return invoke<FsEntry[]>("fs_list_dir", { workspace, relPath });
}

/**
 * S3: what a file is and, for text, its content — decided by Rust so the webview
 * never guesses at a binary and never streams a file it should not render. The
 * size cap lives in Rust: a file above it answers `kind: "too_large"` with its
 * size instead of its bytes.
 */
export function fsReadPreview(
  workspace: string,
  relPath: string,
): Promise<FsPreview> {
  return invoke<FsPreview>("fs_read_preview", { workspace, relPath });
}

/**
 * S3: the bytes of an image preview, RAW.
 *
 * `fs_read_image_bytes` answers `tauri::ipc::Response`, so the payload is a
 * binary body the bridge hands over as an `ArrayBuffer` — not JSON, not base64:
 * no 33 % inflation and no dependency on either side. The caller puts the bytes
 * in a `Blob` with the `mime` `fsReadPreview` reported and revokes the object URL
 * it builds.
 */
export async function fsReadImageBytes(
  workspace: string,
  relPath: string,
): Promise<ArrayBuffer> {
  const answer = await invoke<ArrayBuffer | Uint8Array | number[]>(
    "fs_read_image_bytes",
    { workspace, relPath },
  );
  if (answer instanceof ArrayBuffer) return answer;
  // A bridge that hands over a view or a plain array lands here; normalizing to
  // one `ArrayBuffer` keeps the caller free of `BlobPart` type gymnastics.
  return new Uint8Array(answer).buffer;
}

/**
 * S3: every change of the working tree, staged and unstaged, untracked files
 * included. A folder that is not a git repository answers an EMPTY LIST rather
 * than an error (`src-tauri/src/workspace.rs`), so the two empty faces of the
 * Changes tab share this one answer.
 */
export function gitStatus(workspace: string): Promise<GitEntry[]> {
  return invoke<GitEntry[]>("git_status", { workspace });
}

/**
 * S3: the unified diff of one path against `HEAD` (an untracked file diffs
 * against the empty tree), as git prints it. Loaded per click, never for the
 * whole list, so the Changes tab stays cheap on a large working tree.
 */
export function gitDiff(workspace: string, path: string): Promise<string> {
  return invoke<string>("git_diff", { workspace, path });
}

// ── browser sessions (F3 of `odd/tasks/tool-drawer-and-mcp.md`) ─────────────

/**
 * F3/S6: the Chromium candidates this machine has. An EMPTY list is a valid
 * answer and the one the tab turns into its disabled face: only Chromium speaks
 * CDP, so Safari and Firefox never appear here and the pane must not pretend they
 * can be driven.
 */
export function browserDetect(): Promise<BrowserCandidate[]> {
  return invoke<BrowserCandidate[]>("browser_detect");
}

/**
 * F3: launches a DEDICATED Chromium for the workspace — its own profile and its
 * own `--remote-debugging-port` — instead of attaching to the human's browser.
 * `workspace` is the absolute path the store handed the drawer; Rust owns the
 * argv, the port and the profile directory.
 */
export function browserLaunch(workspace: string): Promise<BrowserSession> {
  return invoke<BrowserSession>("browser_launch", { workspace });
}

/**
 * F3/S4: what the session has open right now — the read-only status the tab
 * polls while it is visible. The pane expects a closed session to answer an empty
 * list and treats a rejection as a real failure (it renders the message in place
 * instead of blanking the status row above it).
 */
export function browserTargets(sessionId: string): Promise<BrowserTarget[]> {
  return invoke<BrowserTarget[]>("browser_targets", { sessionId });
}

/**
 * F3/S4: navigates the session to `url`. The tab only ever passes an `http(s)`
 * URL (it validates before enabling the button), and Rust is expected to reject
 * any other scheme as a second line. Resolves with the new target, or `null` when
 * the browser accepted the navigation but published no target for it — which the
 * tab answers by re-reading `browser_targets` rather than trusting this answer.
 */
export function browserOpenUrl(
  sessionId: string,
  url: string,
): Promise<BrowserTarget | null> {
  return invoke<BrowserTarget | null>("browser_open_url", { sessionId, url });
}

/**
 * F3: kills the browser this app launched and forgets the session. Idempotent:
 * `false` means there was nothing left to close, so unmount never has to guess
 * whether teardown already ran. Never attaches to, or closes, a browser the human
 * started.
 */
export function browserClose(sessionId: string): Promise<boolean> {
  return invoke<boolean>("browser_close", { sessionId });
}
