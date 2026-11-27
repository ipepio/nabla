// Typed wrappers around the Tauri commands (spec S4).
// Vue never talks to pi directly: every read and write goes through Rust.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  open as openDialog,
  save as saveDialog,
} from "@tauri-apps/plugin-dialog";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import type { AgentEventEnvelope } from "./agent-events";
import type {
  AcpAgentStatus,
  AgentsConfig,
  AppSettings,
  CustomAcpAgentDraft,
  CustomVoiceModelDraft,
  EmbedNavEvent,
  EmbedPopupTab,
  FsEntry,
  FsPreview,
  GitEntry,
  HostHardwareInfo,
  ImageAttachment,
  McpAddDraft,
  McpEndpoint,
  McpScope,
  McpServerSummary,
  MemoryEntry,
  ModelInfo,
  ModelOption,
  PermissionMode,
  PiAuthStatus,
  PiCliEvent,
  PiCliResult,
  PreviewMessage,
  PromptImage,
  PdfAttachment,
  ProviderDraft,
  ProviderModel,
  ProviderModelDraft,
  ProviderModels,
  ProviderStatusRow,
  ProviderSummary,
  SessionDto,
  SessionCommand,
  SessionModelInfo,
  SessionSearchHit,
  SessionStatusDto,
  VoiceDownloadProgress,
  VoiceModelEntry,
  VoiceSynthesizeResult,
  VoiceTranscribeResult,
  SessionSummary,
  ShellOutputEvent,
  ShellSession,
  SkillEntry,
  SubagentSession,
  ThinkingLevel,
  TranscriptMessage,
  UiAnswer,
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

/**
 * One structural line from the webview on the dev terminal, through Rust.
 *
 * The webview's own `console` only exists inside the DevTools pane, while `pnpm
 * tauri dev` shows the app process's stderr; this is the seam that puts a UI
 * handler on that terminal. Debug-only on both halves: `import.meta.env.DEV` is
 * folded away in a release build and the Rust command prints nothing there.
 *
 * Callers pass structure — a handler name, a count, a view id, a boolean — never a
 * prompt, a page url or a token, and a failure here is swallowed: a log is never a
 * reason to break a send.
 */
export function debugLog(message: string): void {
  if (!import.meta.env.DEV) return;
  void invoke("debug_log", { message }).catch(() => {});
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
 * S2 of `odd/tasks/permission-modes.md`: the mode the agent is obeying right
 * now, read from `settings.json`. An absent key or a value this build does not
 * know answers `"write"` (the fail-safe default), so the caller never has to
 * interpret an empty string.
 */
export function getPermissionMode(): Promise<PermissionMode> {
  return invoke<PermissionMode>("get_permission_mode");
}

/**
 * S2: sets the mode the human picked in the chat header. Rust persists it in
 * `settings.json` with the existing atomic write and rewrites the projection the
 * loaded pi extension reads on every tool call, which is what makes the change
 * reach a LIVE session: the very next tool call obeys it, with no restart and no
 * new spawn. The answer is what the file now holds, so the chip renders the
 * stored value instead of its own guess.
 *
 * A word that is not one of the three rejects before anything is written: the
 * app never picks a permission level the human did not choose.
 */
export function setPermissionMode(mode: PermissionMode): Promise<PermissionMode> {
  return invoke<PermissionMode>("set_permission_mode", { mode });
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

// ── native folder picker seam (dock, F1 of `tool-drawer-and-mcp.md`) ────────

/**
 * Shows the system's folder picker and answers the chosen absolute path, or
 * `null` when the human dismissed it (which is not an error).
 *
 * Why here: a dock pane is pointed at a folder, and the drawer needs that picker
 * for a path the workspace list does not hold — a second Files pane on another
 * tree, or a shell in another directory. The call lives in the contracts seam for
 * the same reason every other one does (AGENTS.md 1): no component imports the
 * Tauri API directly. It is the `directory: true, multiple: false` shape the
 * sidebar already uses, with the title passed in so both locales apply.
 */
export async function pickDirectory(title: string): Promise<string | null> {
  const selected = await openDialog({ directory: true, multiple: false, title });
  if (selected === null) return null;
  return Array.isArray(selected) ? (selected[0] ?? null) : selected;
}

/**
 * S1 of `odd/tasks/composer-image-input.md`: the composer's `+` opens the system
 * file picker filtered to the formats pi sends inline, and answers the absolute
 * paths (an empty list when the human dismissed it, which is not an error).
 *
 * The files are read by Rust (`readImageAttachment`) and not from the webview: a
 * selected path is a path, and the webview never gets filesystem access.
 */
export async function pickImageFiles(): Promise<string[]> {
  const selected = await openDialog({
    multiple: true,
    directory: false,
    filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "gif", "webp"] }],
  });
  if (selected === null) return [];
  return Array.isArray(selected) ? selected : [selected];
}

/**
 * S3 of `odd/tasks/composer-tools-pdf.md`: the menu's PDF entry opens the same
 * picker filtered to `*.pdf`, and answers the absolute paths (an empty list when
 * the human dismissed it, which is not an error).
 *
 * The file is read and extracted by Rust (`readPdfAttachment`): the webview never
 * opens a PDF, so no parser — and no shell — ever runs inside it.
 */
export async function pickPdfFiles(): Promise<string[]> {
  const selected = await openDialog({
    multiple: true,
    directory: false,
    filters: [{ name: "PDF", extensions: ["pdf"] }],
  });
  if (selected === null) return [];
  return Array.isArray(selected) ? selected : [selected];
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

export function acpListAgents(): Promise<AcpAgentStatus[]> {
  return invoke<AcpAgentStatus[]>("acp_list_agents");
}

export function acpSetAgentEnabled(
  agentId: string,
  enabled: boolean,
): Promise<AcpAgentStatus[]> {
  return invoke<AcpAgentStatus[]>("acp_set_agent_enabled", {
    agentId,
    enabled,
  });
}

export function acpSaveCustomAgent(
  draft: CustomAcpAgentDraft,
): Promise<AcpAgentStatus[]> {
  return invoke<AcpAgentStatus[]>("acp_save_custom_agent", { draft });
}

export function acpDeleteCustomAgent(agentId: string): Promise<AcpAgentStatus[]> {
  return invoke<AcpAgentStatus[]>("acp_delete_custom_agent", { agentId });
}

export function acpStartLogin(
  agentId: string,
  workspace?: string | null,
): Promise<ShellSession> {
  return invoke<ShellSession>("acp_start_login", {
    agentId,
    workspace: workspace ?? null,
  });
}

export function sendPrompt(
  workspaceId: number,
  sessionId: number,
  text: string,
  images: PromptImage[] = [],
): Promise<void> {
  // `images` always crosses as an array: the Rust side takes an optional list, and
  // an empty array is the explicit "text only" the protocol line then omits.
  return invoke<void>("send_prompt", { workspaceId, sessionId, text, images });
}

/**
 * S1-S2 of `odd/tasks/composer-image-input.md`: reads one image file the human
 * picked or dropped, with the format sniffed from its bytes (not from the file
 * extension) and the payload base64-encoded in Rust.
 *
 * Rust is the authority on both: it rejects a file that is not one of pi's inline
 * formats and one that exceeds the size ceiling before sending anything to the
 * webview. The path is one the human picked or dropped — this command never lists
 * directories — and the bytes it may answer with are an image of bounded size, not
 * arbitrary file content.
 */
export function readImageAttachment(path: string): Promise<ImageAttachment> {
  return invoke<ImageAttachment>("read_image_attachment", { path });
}

/**
 * S3 of `odd/tasks/composer-tools-pdf.md`: extracts one PDF's selectable text,
 * locally and page by page, with pi's own runtime never involved.
 *
 * Extraction is Rust's: `pdftotext` runs as an argv-only child (no shell), with the
 * file size, page count, output size and elapsed time bounded there, and the answer
 * preserves page numbering — an empty page stays an empty string in `pages`. A
 * missing `pdftotext` rejects with the install hint, and a document with no
 * selectable text answers `hasText: false` instead of invented text.
 *
 * The path is one the human picked or dropped; this command never lists a
 * directory and answers extracted text, never the PDF's bytes.
 */
export function readPdfAttachment(path: string): Promise<PdfAttachment> {
  return invoke<PdfAttachment>("read_pdf_attachment", { path });
}

export function stopSession(
  workspaceId: number,
  sessionId: number,
): Promise<void> {
  return invoke<void>("stop_session", { workspaceId, sessionId });
}

/**
 * S2 of `odd/tasks/composer-inputs.md`: the thinking levels the session's model
 * accepts, proxied from pi's `get_available_thinking_levels` by Rust. pi answers
 * for the model that is really running, so the effort selector offers exactly
 * what that model can do (a model without reasoning support answers `["off"]`).
 *
 * Rejects when the session has no live process: an empty list would claim the
 * model supports no level at all, which is a different thing from "nothing is
 * running yet".
 */
export function sessionThinkingLevels(
  workspaceId: number,
  sessionId: number,
): Promise<ThinkingLevel[]> {
  return invoke<ThinkingLevel[]>("session_thinking_levels", {
    workspaceId,
    sessionId,
  });
}

/**
 * S2: applies one thinking level to the open session through pi's
 * `set_thinking_level` — no respawn — and keeps it as the new default in
 * `settings.json`, so the next session starts there (`--thinking <level>`).
 *
 * The answer is what the file now holds. The level pi really adopted arrives
 * later as a `thinking_level_changed` event, because pi clamps the request to the
 * model: the selector must render that event, never this answer.
 *
 * A session with no live process only persists the default — there is nothing to
 * change — while a live child that refuses the request fails the call and leaves
 * the file untouched. A word this build does not know rejects before anything is
 * written or sent.
 */
export function setSessionThinkingLevel(
  workspaceId: number,
  sessionId: number,
  level: ThinkingLevel,
): Promise<ThinkingLevel> {
  return invoke<ThinkingLevel>("set_session_thinking_level", {
    workspaceId,
    sessionId,
    level,
  });
}

/**
 * S2 of `odd/tasks/models-by-provider.md`: switches the model of the LIVE session
 * through pi's `set_model` — no respawn — so the effort list and the agent's
 * reasoning really belong to the model the human picked.
 *
 * Model ids are provider-scoped, which is why both travel together. The answer is
 * the model pi says it is now running, so the composer trusts the engine rather
 * than the words it sent. pi re-clamps the thinking level to the new model's
 * range and reports it as `thinking_level_changed`, so nothing here touches the
 * level.
 *
 * Rejects when the session has no live process (spawning or gone): nothing was
 * switched, and a success would claim a change that never happened. The default
 * for the next session is `saveSettings`, which the caller keeps doing on its
 * own.
 */
export function setSessionModel(
  workspaceId: number,
  sessionId: number,
  provider: string,
  modelId: string,
): Promise<SessionModelInfo> {
  return invoke<SessionModelInfo>("set_session_model", {
    workspaceId,
    sessionId,
    provider,
    modelId,
  });
}

/**
 * S3 of `odd/tasks/composer-inputs.md`: the commands the session offers —
 * extension commands, prompt templates and skills — proxied from pi's
 * `get_commands` by Rust. Nabla only lists them: sending the text `/name` through
 * the existing prompt path is what runs one, and pi expands it there.
 *
 * Rejects when the session has no live process; the `/` completion treats that
 * as "nothing to offer yet" and shows no list, never an error.
 */
export function sessionCommands(
  workspaceId: number,
  sessionId: number,
): Promise<SessionCommand[]> {
  return invoke<SessionCommand[]>("session_commands", {
    workspaceId,
    sessionId,
  });
}

/**
 * S3 of `odd/tasks/composer-inputs.md`: the commands a workspace offers, for the
 * `/` completion **before any session exists**.
 *
 * Asked on demand, every app run: Rust spawns a short-lived pi for this
 * workspace, asks it for `get_commands` and kills it, so the list is the
 * workspace's current answer instead of a snapshot someone had to store. The
 * probe never indexes a session, and a probe that cannot be made answers an
 * empty list rather than rejecting — the completion then shows no list, which is
 * the honest answer for "pi could not be asked". Concurrency is Rust's business:
 * two callers for the same workspace share one child.
 */
export function workspaceCommands(
  workspaceId: number,
): Promise<SessionCommand[]> {
  return invoke<SessionCommand[]>("workspace_commands", { workspaceId });
}

/**
 * S3 of `agent-question-bridge.md`: answers one question the session's agent is
 * blocking on. Rust writes the exact `extension_ui_response` line to that
 * child's stdin, pi resolves the extension's promise with the answer, and the
 * turn continues with it.
 *
 * The answer shape is the protocol's own: a `value` for `select`/`input`, a
 * `confirmed` for `confirm`, or `cancelled` for any dialog. Rejects when the
 * session is not running — an answer nobody can receive must not look delivered —
 * and there is nothing else to await: pi sends no response to an answer.
 */
export function answerUiRequest(
  workspaceId: number,
  sessionId: number,
  requestId: string,
  answer: UiAnswer,
): Promise<void> {
  return invoke<void>("answer_ui_request", {
    workspaceId,
    sessionId,
    requestId,
    answer,
  });
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
 * H3 of `odd/tasks/subagent-helpers.md`: the children a chat delegated to, so the
 * sidebar can nest them under the turn that started them.
 *
 * The status is live: Rust re-reads the extension's manifest on every call, so a
 * child that just finished reads as `done` without the caller polling a cache.
 * It also indexes any child it has never seen, which is why opening a chat is
 * enough for its children to appear. Each child's `sessionId` is a normal session
 * id — `sessionTranscript` reads it as-is, because the child's file is a pi JSONL
 * like any other.
 */
export function subagentSessions(
  workspaceId: number,
  parentSessionId: number,
): Promise<SubagentSession[]> {
  return invoke<SubagentSession[]>("subagent_sessions", {
    workspaceId,
    parentSessionId,
  });
}

/**
 * S1 of `odd/tasks/agent-profiles.md`: the subagent profiles, read from Nabla's
 * own `<data dir>/agents.json`. A missing file answers the off state — master
 * switch off, no profiles — and never an error, because that is what a fresh
 * install is.
 */
export function getAgentsConfig(): Promise<AgentsConfig> {
  return invoke<AgentsConfig>("get_agents_config");
}

/**
 * S1/S3/S6: persists the profiles with the same atomic write and the same
 * camelCase contract as `settings.json`. Rust validates first (name present and
 * unique, description present, a known effort, a model that can be a model id) and
 * rejects with a message that names the profile, so the dialog shows exactly what
 * the human has to fix instead of writing a file the extension would have to
 * guess about.
 *
 * The answer is what the file now holds, not what was sent.
 */
export function saveAgentsConfig(config: AgentsConfig): Promise<AgentsConfig> {
  return invoke<AgentsConfig>("save_agents_config", { config });
}

/**
 * The preview of a session: its newest turns, with the reasoning pi's JSONL file
 * never stored, so an open paints before the engine boots.
 *
 * `null` means there is nothing worth painting yet and the caller reads pi's file
 * instead — no snapshot, a snapshot that no longer parses, or one pi's own file
 * has outgrown. Rust decides that freshness; it is never an error.
 */
export function loadSessionPreview(
  sessionId: number,
): Promise<PreviewMessage[] | null> {
  return invoke<PreviewMessage[] | null>("load_session_preview", { sessionId });
}

/**
 * Snapshots the tail of a session's conversation so the next open paints from it.
 *
 * Best-effort by contract: `false` means nothing was stored (the session is not
 * indexed, or the write failed) and the caller carries on — pi's file stays the
 * transcript, this is only a head start on painting it.
 */
export function saveSessionPreview(
  sessionId: number,
  messages: PreviewMessage[],
): Promise<boolean> {
  return invoke<boolean>("save_session_preview", { sessionId, messages });
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

// ── embedded browser (a child webview the app places and drives) ────────────
//
// A child webview FLOATS ABOVE the HTML: the pane reserves the rectangle it
// covers instead of hosting it. Every coordinate and size below is in LOGICAL
// pixels, which is what `getBoundingClientRect()` already answers in this
// webview, so the pane passes the measured rect through unchanged. `src-tauri/
// src/embed.rs` documents the platform story and the url policy.

/**
 * Creates a child webview of the main window covering the given rect and answers
 * its id (`embed-<n>`). `url` defaults to `about:blank`; only `http`, `https` and
 * `about:blank` are accepted, and anything else is refused by Rust.
 */
export function embedCreate(
  x: number,
  y: number,
  width: number,
  height: number,
  url: string | null = null,
): Promise<string> {
  return invoke<string>("embed_create", { x, y, width, height, url });
}

/**
 * Moves and resizes one view. The pane calls this with the measured body rect on
 * every resize, so the child webview keeps covering exactly that rect.
 */
export function embedSetBounds(
  viewId: string,
  x: number,
  y: number,
  width: number,
  height: number,
): Promise<void> {
  return invoke<void>("embed_set_bounds", { viewId, x, y, width, height });
}

/**
 * Shows or hides one view without closing it.
 *
 * A child webview floats above the HTML, so CSS cannot hide it: this is the
 * platform's own switch, and it keeps the page. The pane calls it with `false`
 * when the drawer collapses and with `true` when it reopens — the same page comes
 * back, never a reload — and both directions are idempotent.
 */
export function embedSetVisible(
  viewId: string,
  visible: boolean,
): Promise<void> {
  return invoke<void>("embed_set_visible", { viewId, visible });
}

/**
 * Navigates one view. The pane validates locally first and only ever passes an
 * `http(s)` or `about:blank` url; Rust refuses anything else as a second line.
 */
export function embedNavigate(viewId: string, url: string): Promise<void> {
  return invoke<void>("embed_navigate", { viewId, url });
}

/**
 * Evaluates `script` in one view and answers the JSON string the platform
 * serializes the result into. Used for the page-level controls (history and
 * reload), which have no command of their own.
 */
export function embedEval(viewId: string, script: string): Promise<string> {
  return invoke<string>("embed_eval", { viewId, script });
}

/** Closes one view. Idempotent: `false` means there was nothing left to close. */
export function embedClose(viewId: string): Promise<boolean> {
  return invoke<boolean>("embed_close", { viewId });
}

/** The last url one view navigated to: the page the human is looking at. */
export function embedUrl(viewId: string): Promise<string> {
  return invoke<string>("embed_url", { viewId });
}

/**
 * The popups the platform has finished building, each with the view that opened it.
 *
 * The pane adopts them as tabs of its own strip (`embed-popup` is the live signal;
 * this is the reconciliation, for a popup that arrived while no listener was up).
 * A popup is never an agent target: Rust keeps it out of the list the MCP bridge
 * gives the model.
 */
export function embedPopups(): Promise<EmbedPopupTab[]> {
  return invoke<EmbedPopupTab[]>("embed_popups");
}

/**
 * Subscribes to the `embed-nav` stream. One listener serves every view, so the
 * caller filters by its own `viewId`; the listener lives here because no
 * component imports the Tauri API directly (contract-seam rule, AGENTS.md 1).
 * Resolves with an unsubscribe function: call it on unmount or it keeps receiving
 * events after teardown.
 */
export async function subscribeEmbedNav(
  handler: (event: EmbedNavEvent) => void,
): Promise<() => void> {
  return listen<EmbedNavEvent>("embed-nav", ({ payload }) => {
    handler(payload);
  });
}

/**
 * Subscribes to `embed-popup`: a page asked for a new window and the popup is now a
 * hidden child webview the pane can adopt as a tab. The payload carries ids only
 * (see `EmbedPopupTab`). The listener lives here for the same reason as the ones
 * above (contract-seam rule, AGENTS.md 1). Resolves with an unsubscribe function:
 * call it on unmount or it keeps receiving events after teardown.
 */
export async function subscribeEmbedPopup(
  handler: (popup: EmbedPopupTab) => void,
): Promise<() => void> {
  return listen<EmbedPopupTab>("embed-popup", ({ payload }) => {
    handler(payload);
  });
}

/**
 * Subscribes to `embed-need-view`: the MCP bridge telling the app that a browser
 * tool needs a view and that there is none (`auto-open-pane.md`, S1/S3). There is
 * no payload — the request is only "a view is needed" — and the app answers by
 * opening the browser pane, because the rect the view is created with is geometry
 * only this webview can measure. The listener lives here for the same reason as
 * the ones above (contract-seam rule, AGENTS.md 1). Resolves with an unsubscribe
 * function: call it on unmount or it keeps receiving events after teardown.
 */
export async function subscribeEmbedNeedView(
  handler: () => void,
): Promise<() => void> {
  return listen("embed-need-view", () => {
    handler();
  });
}

/**
 * T3 of `webview-control.md`: the local MCP endpoint this run offers so the agent
 * can drive those views, or `null` when the port could not be bound — the app
 * works the same, only without agent control of the browser (S6).
 *
 * Nabla registers the endpoint with pi itself, so nothing in the UI needs this
 * to work; the wrapper keeps the seam symmetric, with the payload, its command and
 * this function moving together.
 */
export function mcpEndpoint(): Promise<McpEndpoint | null> {
  return invoke<McpEndpoint | null>("mcp_endpoint");
}

/** Renames a workspace alias in SQLite (path stays unchanged). */
export function renameWorkspace(id: number, name: string): Promise<WorkspaceInfo> {
  return invoke<WorkspaceInfo>("rename_workspace", { id, name });
}

/** Renames a session title in SQLite (pi's JSONL stays unchanged). */
export function renameSession(sessionId: number, title: string): Promise<void> {
  return invoke<void>("rename_session", { sessionId, title });
}

/** Full-text FTS5 search across past session transcripts (rebuildable from JSONL). */
export function searchSessions(
  query: string,
  limit = 12,
): Promise<SessionSearchHit[]> {
  return invoke<SessionSearchHit[]>("search_sessions", { query, limit });
}

/** Lists local `.md` prompt skills from global config and optional workspace. */
export function listSkills(workspaceId: number | null): Promise<SkillEntry[]> {
  return invoke<SkillEntry[]>("list_skills", { workspaceId });
}

/** Saves or updates a global `.md` prompt skill. */
export function saveSkill(payload: {
  name: string;
  description: string;
  prompt: string;
}): Promise<SkillEntry> {
  return invoke<SkillEntry>("save_skill", payload);
}

/** Deletes a global `.md` prompt skill by slug name. */
export function deleteSkill(name: string): Promise<boolean> {
  return invoke<boolean>("delete_skill", { name });
}

/** Sends a best-effort native OS notification when Nabla is in the background. */
export function notifyDesktop(title: string, body: string): Promise<void> {
  return invoke<void>("notify_desktop", { title, body });
}

/** Reads a Markdown document from disk for the Document drawer tool. */
export function docRead(path: string): Promise<string> {
  return invoke<string>("doc_read", { path });
}

/** Saves a Markdown document to disk from the Document drawer tool. */
export function docWrite(path: string, content: string): Promise<void> {
  return invoke<void>("doc_write", { path, content });
}

/** Creates a transparent scratch Markdown file in Nabla's tmp folder. */
export function docCreateTemp(): Promise<string> {
  return invoke<string>("doc_create_temp");
}

/** Exports a Markdown file to `.docx` (embedding local PNG/JPEG images). */
export function exportDocxFile(
  input: string,
  output: string | null = null,
): Promise<string> {
  return invoke<string>("export_docx_file", { input, output });
}

/** Opens a native picker for a single Markdown file. */
export async function pickMarkdownFile(
  title: string,
  defaultPath?: string,
): Promise<string | null> {
  const picked = await openDialog({
    title,
    defaultPath,
    directory: false,
    multiple: false,
    filters: [{ name: "Markdown", extensions: ["md", "markdown", "txt"] }],
  });
  return typeof picked === "string" ? picked : null;
}

/** Opens a native Save dialog for creating a new Markdown file. */
export async function pickSaveMarkdownFile(
  title: string,
  defaultPath?: string,
): Promise<string | null> {
  const picked = await saveDialog({
    title,
    defaultPath,
    filters: [{ name: "Markdown", extensions: ["md"] }],
  });
  return typeof picked === "string" ? picked : null;
}

/** Opens a native Save dialog for exporting a `.docx` file. */
export async function pickSaveDocxFile(
  title: string,
  defaultPath?: string,
): Promise<string | null> {
  const picked = await saveDialog({
    title,
    defaultPath,
    filters: [{ name: "Word Document", extensions: ["docx"] }],
  });
  return typeof picked === "string" ? picked : null;
}

/** Subscribes to `doc-open` events emitted when an agent opens/exports a doc. */
export async function subscribeDocOpen(
  handler: (payload: { path: string }) => void,
): Promise<() => void> {
  return listen<{ path: string }>("doc-open", ({ payload }) => {
    handler(payload);
  });
}

/** Subscribes to `drawer-open-tool` events emitted when an agent opens a drawer tool. */
export async function subscribeDrawerOpenTool(
  handler: (payload: { tool: string; path?: string | null }) => void,
): Promise<() => void> {
  return listen<{ tool: string; path?: string | null }>("drawer-open-tool", ({ payload }) => {
    handler(payload);
  });
}

/** Lists or FTS5-searches persistent agent memories from `memory.db`. */
export function memoryList(
  query: string | null = null,
  scope: string | null = null,
  limit = 50,
): Promise<MemoryEntry[]> {
  return invoke<MemoryEntry[]>("memory_list", { query, scope, limit });
}

/** Saves or updates a persistent agent memory in `memory.db`. */
export function memorySave(payload: {
  title: string;
  content: string;
  kind?: string | null;
  tags?: string | null;
  scope?: string | null;
}): Promise<MemoryEntry> {
  return invoke<MemoryEntry>("memory_save", {
    title: payload.title,
    content: payload.content,
    kind: payload.kind ?? null,
    tags: payload.tags ?? null,
    scope: payload.scope ?? null,
  });
}

/** Deletes a single persistent memory by id. */
export function memoryDelete(id: number): Promise<boolean> {
  return invoke<boolean>("memory_delete", { id });
}

/** Clears persistent memories (all, or filtered by scope). */
export function memoryClear(scope: string | null = null): Promise<number> {
  return invoke<number>("memory_clear", { scope });
}

/** Signals Nabla's subagent extension to abort one specific child session. */
export function cancelSubagent(sessionPath: string): Promise<void> {
  return invoke<void>("cancel_subagent", { sessionPath });
}

// ── local voice models (embedded CPU TTS & STT) ─────────────────────────────

export function voiceListModels(): Promise<VoiceModelEntry[]> {
  return invoke<VoiceModelEntry[]>("voice_list_models");
}

export function voiceHostInfo(): Promise<HostHardwareInfo> {
  return invoke<HostHardwareInfo>("voice_host_info");
}

export function voiceDownloadModel(
  modelId: string,
  voiceIds?: string[],
): Promise<void> {
  return invoke<void>("voice_download_model", {
    modelId,
    voiceIds: voiceIds ?? null,
  });
}

export function voiceCancelDownload(modelId: string): Promise<boolean> {
  return invoke<boolean>("voice_cancel_download", { modelId });
}

export function voiceDeleteModel(
  modelId: string,
  voiceId?: string,
): Promise<boolean> {
  return invoke<boolean>("voice_delete_model", {
    modelId,
    voiceId: voiceId ?? null,
  });
}

export function voiceImportCustomModel(
  draft: CustomVoiceModelDraft,
): Promise<VoiceModelEntry> {
  return invoke<VoiceModelEntry>("voice_import_custom_model", { draft });
}

export async function subscribeVoiceDownloadProgress(
  handler: (progress: VoiceDownloadProgress) => void,
): Promise<() => void> {
  return listen<VoiceDownloadProgress>("voice-download-progress", ({ payload }) => {
    handler(payload);
  });
}

export function voiceTranscribePcm(payload: {
  pcmBase64: string;
  sampleRate: number;
  modelId?: string | null;
}): Promise<VoiceTranscribeResult> {
  return invoke<VoiceTranscribeResult>("voice_transcribe_pcm", {
    pcmBase64: payload.pcmBase64,
    sampleRate: payload.sampleRate,
    modelId: payload.modelId ?? null,
  });
}

export function voiceSynthesizeSegment(payload: {
  text: string;
  modelId?: string | null;
  voiceId?: string | null;
  speed?: number | null;
  language?: string | null;
}): Promise<VoiceSynthesizeResult> {
  return invoke<VoiceSynthesizeResult>("voice_synthesize_segment", {
    text: payload.text,
    modelId: payload.modelId ?? null,
    voiceId: payload.voiceId ?? null,
    speed: payload.speed ?? null,
    language: payload.language ?? null,
  });
}

export function voiceRecordStart(): Promise<void> {
  return invoke<void>("voice_record_start");
}

export function voiceRecordStopAndTranscribe(): Promise<VoiceTranscribeResult> {
  return invoke<VoiceTranscribeResult>("voice_record_stop_and_transcribe");
}

export function voiceRecordCancel(): Promise<void> {
  return invoke<void>("voice_record_cancel");
}
