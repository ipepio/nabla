// Command layer: the only seam between the webview and Rust (S4).
// Rule: "Vue never talks to pi directly" — every read or write crosses this
// module, which reads and writes the store/settings owned by the app. Payloads
// are camelCase so the TS contracts (T6) can mirror them field by field.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use tauri::State;
use tauri_plugin_opener::OpenerExt;

use crate::cli::{self, CliSessions, CliSink, PiAuthStatus, PiCliResult};
use crate::image_input::{self, ImageAttachment, PromptImage};
use crate::pdf_input::PdfAttachment;
use crate::mcp::{self, McpAddDraft, McpScope, McpServerSummary};
use crate::mcp_server::{REGISTRATION_WAIT, RegistrationGate, RegistrationOutcome};
use crate::process::{ProcessManager, SessionModel, StartupOutcome};
use crate::providers::{
    ModelInfo as ProviderModelInfo, ModelOption, ProviderDraft, ProviderModelDraft, ProviderModels,
    ProviderSummary,
};
use crate::rpc::{AgentEvent, RuntimeStatus, UiAnswer};
use crate::settings::{self, PermissionMode, Settings};
use crate::shell::{self, ShellSession, ShellSessions, ShellSink};
use crate::store::{ProviderStatusRow, SessionNew, SessionRow, Store, StoreResult, Workspace};
use crate::workspace::{FsEntry, FsPreview, GitEntry};

/// State shared with every command: the SQLite store (app-owned state), the
/// config directory where settings.json lives (human-owned preferences, S2), the
/// app data directory that holds the pi-owned session files (S7), and the pi
/// process supervisor (S1, S3).
pub struct AppState {
    pub store: Store,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    /// S3 of `providers-pane.md`: pi's own provider configuration lives under the
    /// user's home directory, resolved once at startup by Tauri's path resolver.
    pub home_dir: PathBuf,
    pub runtime: ProcessManager,
    /// S1/S3 of `pi-cli-bridge.md`: the allowlisted bridge to the `pi` command
    /// line and the registry of interactive sessions it started.
    pub cli: CliSessions,
    /// Emitter of the `pi-cli` event. The registry receives the sink per call,
    /// so nothing below this layer decides how an event reaches the webview.
    pub cli_sink: CliSink,
    /// S2 of `tool-drawer-and-mcp.md` (F2): the workspace terminals, one pty per
    /// tab, and the sink their `shell-output` events leave through.
    pub shells: ShellSessions,
    pub shell_sink: ShellSink,
    /// The embedded browser: one child webview per view, added to the main
    /// window, plus the last url each one reported. Tauri owns the webviews;
    /// this registry is the handle the commands address.
    pub embeds: crate::embed::EmbedSessions,
    /// `webview-control.md` T1: the local MCP server the agent connects to in
    /// order to drive those views, and the token of this run. `None` when the
    /// port could not be bound — the app works the same, only without agent
    /// control of the browser.
    pub mcp_endpoint: Option<crate::mcp_server::Endpoint>,
    /// S4 of `webview-control.md`: whether this run's endpoint reached pi's
    /// `mcp.json`, and the latch a session start waits on before spawning pi.
    ///
    /// pi reads that file once, when it starts (L8), so a session spawned before
    /// the registration lands inherits a config without `nabla-browser` and keeps
    /// it for its whole life. The gate is what makes that window impossible, and
    /// it keeps the outcome afterwards instead of swallowing it.
    pub mcp_registration: Arc<RegistrationGate>,
    /// The in-flight command probes, one per workspace: the guard that keeps a
    /// workspace from ever owning two probe children at once. Per app instance,
    /// never a process-wide static, so two stores can never share a slot.
    pub probes: Arc<CommandProbes>,
}

// Errors cross the bridge as plain strings for now: StoreError and SettingsError
// are mapped through Display. Typed error payloads, with a matching TS contract,
// arrive together with the runtime feature; nothing here freezes an error shape.
fn display_err<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

/// A mutex whose lock is taken even when the guard was poisoned: the state
/// behind these locks is replaced wholesale, so a poison flag left by an
/// unrelated panic must not turn every later caller into a failure.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Path of the user-editable preferences file inside a config directory.
/// `pub(crate)` because the setup hook resolves it once at boot to project the
/// permission mode for the loaded pi extension (`permission-modes.md`, L4).
pub(crate) fn settings_file(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

/// Path of the user-editable preferences file inside the app config directory.
fn settings_path(state: &AppState) -> PathBuf {
    settings_file(&state.config_dir)
}

#[tauri::command]
pub fn list_workspaces(state: State<'_, AppState>) -> Result<Vec<Workspace>, String> {
    state.store.list_workspaces().map_err(display_err)
}

#[tauri::command]
pub fn add_workspace(
    state: State<'_, AppState>,
    path: String,
    name: Option<String>,
) -> Result<Workspace, String> {
    // A workspace is a directory on disk; indexing one that does not exist would
    // produce rows no later command could use.
    if !Path::new(&path).is_dir() {
        return Err(format!(
            "workspace path is not an existing directory: {path}"
        ));
    }
    state
        .store
        .add_workspace(&path, name.as_deref())
        .map_err(display_err)
}

/// Seeds the user's home directory as the default workspace on the first launch
/// (S9: "un workspace por default que sea ~").
///
/// Runs from the Tauri setup hook, not from a command: it must happen before the
/// webview asks for the workspace list. Idempotent by the emptiness check (later
/// launches keep whatever the human configured) and, as a second belt, by the
/// store's `path UNIQUE` constraint. Returns whether a row was created.
pub fn seed_default_workspace(store: &Store, home: &Path) -> StoreResult<bool> {
    if !store.list_workspaces()?.is_empty() {
        return Ok(false);
    }
    // `name` is omitted on purpose: the store derives it from the path.
    store.add_workspace(&home.to_string_lossy(), None)?;
    Ok(true)
}

#[tauri::command]
pub fn remove_workspace(state: State<'_, AppState>, id: i64) -> Result<bool, String> {
    state.store.remove_workspace(id).map_err(display_err)
}

/// Message for an id that is not (or no longer) indexed, shared by every command
/// that takes a workspace id so the wording never drifts.
fn unknown_workspace(workspace_id: i64) -> String {
    format!("unknown workspace: {workspace_id}")
}

/// Reads one indexed workspace. Commands that mutate a workspace answer with the
/// row they just wrote, so the UI never has to guess what the update produced.
fn workspace_by_id(store: &Store, workspace_id: i64) -> Result<Workspace, String> {
    store
        .list_workspaces()
        .map_err(display_err)?
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
        .ok_or_else(|| unknown_workspace(workspace_id))
}

/// S8: pins or unpins one space for the "Pinned" filter. Returns the updated row;
/// an id that changes no row is an error, not a silent success.
#[tauri::command]
pub fn set_workspace_pinned(
    state: State<'_, AppState>,
    workspace_id: i64,
    pinned: bool,
) -> Result<Workspace, String> {
    let changed = state
        .store
        .set_workspace_pinned(workspace_id, pinned)
        .map_err(display_err)?;
    if !changed {
        return Err(unknown_workspace(workspace_id));
    }
    workspace_by_id(&state.store, workspace_id)
}

/// S8: records that the space was just used ("Recientes"). Returns the updated
/// row; an id that changes no row is an error, not a silent success.
#[tauri::command]
pub fn touch_workspace(
    state: State<'_, AppState>,
    workspace_id: i64,
) -> Result<Workspace, String> {
    let changed = state
        .store
        .touch_workspace(workspace_id)
        .map_err(display_err)?;
    if !changed {
        return Err(unknown_workspace(workspace_id));
    }
    workspace_by_id(&state.store, workspace_id)
}

#[tauri::command]
pub fn list_sessions(
    state: State<'_, AppState>,
    workspace_id: i64,
) -> Result<Vec<SessionRow>, String> {
    state.store.list_sessions(workspace_id).map_err(display_err)
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    settings::load(&settings_path(&state)).map_err(display_err)
}

// ── subagent profiles seam (S1, S3, S6 of `agent-profiles.md`) ──────────────

/// S1: the profile store is Nabla's, beside `settings.json` and
/// `permissions.json`, never a file inside the project.
fn agents_path(state: &AppState) -> PathBuf {
    crate::agents::config_path(&state.data_dir)
}

/// S1/S3: the profiles as the file holds them. A missing file is the off state
/// (master switch off, no profiles), so a fresh install opens the dialog on
/// nothing enabled instead of on an error.
#[tauri::command]
pub fn get_agents_config(state: State<'_, AppState>) -> Result<crate::agents::AgentsConfig, String> {
    crate::agents::load(&agents_path(&state)).map_err(display_err)
}

/// S1/S6: validates and writes the profiles atomically, then answers what the
/// file actually holds — the same contract `save_settings` honours, so the
/// dialog renders the persisted shape instead of its own guess.
#[tauri::command]
pub fn save_agents_config(
    state: State<'_, AppState>,
    config: crate::agents::AgentsConfig,
) -> Result<crate::agents::AgentsConfig, String> {
    let path = agents_path(&state);
    crate::agents::save(&path, &config).map_err(display_err)?;
    crate::agents::load(&path).map_err(display_err)
}

// ── model selector seam (S7) ────────────────────────────────────────────────

/// One choice of the model selector (S7): the id pi is spawned with and the
/// human-readable label. camelCase like every other payload.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
}

/// Models of the NaN provider, in the order the selector shows them.
/// Source: `~/.pi/agent/models.json`, `providers.nan` (id, name). This is a
/// durable constant on purpose: the selector must work without reading pi's own
/// configuration. Replace the constant with dynamic provider discovery when pi
/// exposes it.
const NAN_MODELS: &[(&str, &str)] = &[
    ("mimo-v2.6-flash", "MiMo V2.6 Flash (NaN)"),
    ("glm5.3-flash", "GLM 5.3 Flash (NaN)"),
    ("gemma4", "Gemma4 (NaN)"),
    ("qwen3.6", "qwen3.6 (NaN)"),
    ("qwen3.8-flash", "Qwen 3.8 Flash (NaN)"),
    ("deepseek-v4-flash", "deepseek (NaN)"),
    ("hy4", "Hy4 (NaN)"),
    ("deepseek-v4-pro", "DeepSeek V4 Pro (NaN)"),
    ("glm5.3", "GLM 5.3 (NaN)"),
    ("qwen3.8-max", "Qwen 3.8 Max (NaN)"),
    ("kimi-k3.1-pro", "Kimi K3.1 Pro (NaN)"),
];

/// S7: the models the selector offers. Synchronous and stateless on purpose:
/// the list is fixed until provider discovery exists.
#[tauri::command]
pub fn list_models() -> Result<Vec<ModelInfo>, String> {
    Ok(NAN_MODELS
        .iter()
        .map(|(id, name)| ModelInfo {
            id: id.to_string(),
            name: name.to_string(),
        })
        .collect())
}

// ── providers seam (S1-S4 of `providers-pane.md`) ───────────────────────────

/// Path of pi's provider configuration inside the user's home directory. One
/// resolver for reading and writing, so both target the `models.json` pi itself
/// reads (S3).
fn models_file(home_dir: &Path) -> PathBuf {
    crate::providers::models_path(home_dir)
}

fn models_path(state: &AppState) -> PathBuf {
    models_file(&state.home_dir)
}

/// Path of pi's cached provider catalog. Read-only: the union of
/// `list_provider_models` needs it, and nothing here writes it.
fn catalog_file(home_dir: &Path) -> PathBuf {
    crate::providers::models_store_path(home_dir)
}

fn catalog_path(state: &AppState) -> PathBuf {
    catalog_file(&state.home_dir)
}

/// Path of pi's credentials file. Read-only too: it decides which catalog-only
/// providers are usable, and no token ever leaves `providers`.
fn credentials_file(home_dir: &Path) -> PathBuf {
    crate::providers::auth_path(home_dir)
}

fn credentials_path(state: &AppState) -> PathBuf {
    credentials_file(&state.home_dir)
}

/// S1: the providers pi knows. The api key is reduced to `has_api_key` (S4).
#[tauri::command]
pub fn list_providers(state: State<'_, AppState>) -> Result<Vec<ProviderSummary>, String> {
    crate::providers::read_providers(&models_path(&state)).map_err(display_err)
}

/// S3: backup, merge into `providers.<id>` and atomic write; answers with the
/// summary of what the file now holds.
#[tauri::command]
pub fn save_provider(
    state: State<'_, AppState>,
    draft: ProviderDraft,
) -> Result<ProviderSummary, String> {
    crate::providers::save_provider(&models_path(&state), &draft).map_err(display_err)
}

/// S2: `GET {baseUrl}/models` from Rust, so the api key never reaches the
/// webview and CORS/CSP stay out of the way.
#[tauri::command]
pub async fn discover_models(
    base_url: String,
    api_key: String,
) -> Result<Vec<ModelOption>, String> {
    crate::providers::discover_models(&base_url, &api_key)
        .await
        .map_err(display_err)
}

/// S2/S4: refreshes the model list of a provider that is already in pi's file.
///
/// The editor sends only the id: the base url and the api key are read from
/// `~/.pi/agent/models.json` inside Rust, so the key never crosses into the
/// webview just to list models. An unknown id, a non-http url and a remote
/// provider without a key are errors before any request; a provider on the
/// machine itself (`localhost`, `127.0.0.1`, `::1`) needs no key, which is the
/// normal case for llama.cpp, Ollama or a local vLLM. The answer is the same
/// `[{ id, name }]` the create form's discovery returns.
#[tauri::command]
pub async fn discover_provider_models(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<Vec<ModelOption>, String> {
    crate::providers::discover_provider_models(&models_path(&state), &provider_id)
        .await
        .map_err(display_err)
}

/// S3: removes one provider from pi's configuration, backup and atomic write
/// included. `true` means an entry was removed; an id the file does not have is
/// `false` and not an error, which keeps the delete idempotent (the same stance
/// `list_providers` takes: it never invents an entry). A malformed id is still
/// rejected before the file is touched.
#[tauri::command]
pub fn delete_provider(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<bool, String> {
    crate::providers::delete_provider(&models_path(&state), &provider_id).map_err(display_err)
}

/// S4: sets the api key of an existing provider in place, in pi's plain-text
/// file (the user's own decision). The entry is never created here and the
/// answer is the summary `list_providers` would report, so the pane refreshes
/// from what the file holds without ever receiving the key back.
#[tauri::command]
pub fn set_provider_key(
    state: State<'_, AppState>,
    provider_id: String,
    api_key: String,
) -> Result<ProviderSummary, String> {
    crate::providers::set_provider_key(&models_path(&state), &provider_id, &api_key)
        .map_err(display_err)
}

// ── models of one provider (S2, S3 of `models-by-provider.md`) ──────────────

/// S2: every provider with its models, read from pi's own `models.json` plus the
/// catalog pi caches in `models-store.json`.
///
/// This is what the grouped selector and the per-model editor are built on: a
/// provider with no models comes back with an empty list, a provider pi signed
/// in without ever writing `models.json` (openai, openai-codex) comes back from
/// the catalog, and the api key never crosses (the DTO has no field for it).
/// Every model says which file it came from (`source`: `configured` or
/// `catalog`), which is what tells the UI whether it may be edited. A missing or
/// corrupt catalog is not an error: it only contributes no catalog providers.
///
/// S1/S2 of `model-visibility.md`: a catalogue-only provider is listed only when
/// pi's `auth.json` proves the human signed in to it (the cache alone is not
/// evidence), and every model carries `hidden`, read from this app's settings so
/// the selector can leave a hidden model out. A corrupt settings file cannot blank
/// the listing: it only hides nothing.
#[tauri::command]
pub fn list_provider_models(
    state: State<'_, AppState>,
) -> Result<Vec<ProviderModels>, String> {
    let mut providers = crate::providers::read_provider_models_with_catalog(
        &models_path(&state),
        &catalog_path(&state),
        &credentials_path(&state),
    )
    .map_err(display_err)?;
    let hidden = settings::load(&settings_path(&state))
        .map(|settings| settings.hidden_models)
        .unwrap_or_default();
    crate::providers::apply_hidden_models(&mut providers, &hidden);
    Ok(providers)
}

/// S2/S3 of `model-visibility.md`: hides or unhides one model of one provider.
///
/// Hiding is Nabla's own setting, written to `settings.json` through the same
/// writer every other preference uses: pi's `models.json` and cached catalogue
/// are read-only, so a catalogue model is hideable exactly like a configured one.
/// The answer is whether the file changed, so hiding twice (or unhiding a visible
/// model) is a successful no-op that rewrites nothing. A blank provider or model
/// id is rejected before the file is touched.
#[tauri::command]
pub fn set_model_hidden(
    state: State<'_, AppState>,
    provider: String,
    model_id: String,
    hidden: bool,
) -> Result<bool, String> {
    settings::set_hidden_model(&settings_path(&state), &provider, &model_id, hidden)
        .map_err(display_err)
}

/// S3: writes one model inside `providers.<provider>.models` — backup, merge and
/// atomic write. The answer is the entry as the file now holds it, so the editor
/// refreshes from what was written. The provider must already exist; nothing is
/// invented here.
#[tauri::command]
pub fn update_provider_model(
    state: State<'_, AppState>,
    draft: ProviderModelDraft,
) -> Result<ProviderModelInfo, String> {
    crate::providers::update_provider_model(&models_path(&state), &draft).map_err(display_err)
}

/// S3: removes one model from one provider, backup and atomic write included.
/// `true` means an entry was removed; a provider or model the file does not have
/// is `false`, which keeps the delete idempotent. A malformed id is still
/// rejected before the file is touched.
#[tauri::command]
pub fn remove_provider_model(
    state: State<'_, AppState>,
    provider: String,
    model_id: String,
) -> Result<bool, String> {
    crate::providers::remove_provider_model(&models_path(&state), &provider, &model_id)
        .map_err(display_err)
}

#[tauri::command]
pub fn save_settings(
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Settings, String> {
    let path = settings_path(&state);
    // `settings` is the payload here, so the module is reached by crate path.
    crate::settings::save(&path, &settings).map_err(display_err)?;
    // Return what the file actually holds, not what was sent: the caller sees
    // the persisted shape (round-tripped through defaults/normalization).
    let saved = crate::settings::load(&path).map_err(display_err)?;
    // S2 of `permission-modes.md`: this file is one of the ways the mode changes,
    // so the projection the loaded gate reads follows it here too (L4) instead of
    // only on the dedicated command below.
    crate::settings::write_permission_projection(&state.data_dir, saved.mode())
        .map_err(display_err)?;
    Ok(saved)
}

// ── permission modes seam (S1-S6 of `permission-modes.md`) ─────────────────

/// S1/S2: the mode the app is in, read from `settings.json`. An absent key (a
/// file written before this preference existed) or a value this build does not
/// know answers `write`, the fail-safe default of S6.
fn permission_mode_for(config_dir: &Path) -> Result<String, String> {
    let settings = settings::load(&settings_file(config_dir)).map_err(display_err)?;
    Ok(settings.mode().as_str().to_string())
}

/// S1/S2: persists one mode and rewrites the projection the loaded extension
/// reads on every tool call (L4). That rewrite is what makes a change reach a
/// LIVE session: the environment was fixed at spawn and is never consulted, so
/// the next tool call already obeys the new mode without a restart.
///
/// A word that is not one of the three is rejected before anything is written:
/// guessing here would silently pick a permission level on the human's behalf.
fn set_permission_mode_for(
    config_dir: &Path,
    data_dir: &Path,
    mode: &str,
) -> Result<String, String> {
    let parsed = PermissionMode::parse(mode).ok_or_else(|| {
        format!(
            "unknown permission mode: {mode:?} (expected \"read\", \"write\" or \"yolo\")"
        )
    })?;
    let path = settings_file(config_dir);
    let mut settings = settings::load(&path).map_err(display_err)?;
    settings.permission_mode = parsed.as_str().to_string();
    settings::save(&path, &settings).map_err(display_err)?;
    settings::write_permission_projection(data_dir, parsed).map_err(display_err)?;
    Ok(parsed.as_str().to_string())
}

/// L4: writes `<data_dir>/permissions.json` from whatever the settings file
/// holds. Called at boot, before any child exists, so the gate reads a real mode
/// from its very first tool call.
///
/// A settings file that does not parse still refreshes the projection, with the
/// fail-safe `write`: leaving an older — possibly `yolo` — projection behind
/// would let a broken file uncage the agent.
pub fn sync_permission_projection(
    config_dir: &Path,
    data_dir: &Path,
) -> Result<PermissionMode, String> {
    match settings::load(&settings_file(config_dir)) {
        Ok(settings) => {
            let mode = settings.mode();
            settings::write_permission_projection(data_dir, mode).map_err(display_err)?;
            Ok(mode)
        }
        Err(err) => {
            settings::write_permission_projection(data_dir, PermissionMode::Write)
                .map_err(display_err)?;
            Err(display_err(err))
        }
    }
}

/// S1/S2: the current mode, for the chip in the chat header.
#[tauri::command]
pub fn get_permission_mode(state: State<'_, AppState>) -> Result<String, String> {
    permission_mode_for(&state.config_dir)
}

/// S1/S2: the human's new mode. Persisted in `settings.json` with the existing
/// atomic write, projected for the live gate, and answered as what the file now
/// holds — so the chip renders the stored value instead of its own guess.
#[tauri::command]
pub fn set_permission_mode(
    state: State<'_, AppState>,
    mode: String,
) -> Result<String, String> {
    set_permission_mode_for(&state.config_dir, &state.data_dir, &mode)
}

// ── configuration file seam (S2 of `settings-dialog.md`) ────────────────────

/// S2: the absolute path of the preferences file the settings dialog shows.
/// `settings_file` stays the single resolver, so the path the UI displays is the
/// same file `get_settings`/`save_settings` read and write.
fn config_file_path_for(config_dir: &Path) -> Result<String, String> {
    let path = settings_file(config_dir);
    // Tauri resolves the config dir absolutely; a relative one would display a
    // path that does not name the file the app opens.
    if !path.is_absolute() {
        return Err(format!("settings path is not absolute: {}", path.display()));
    }
    Ok(path.to_string_lossy().to_string())
}

/// S2: the preferences file the "Open configuration file" action targets.
#[tauri::command]
pub fn config_file_path(state: State<'_, AppState>) -> Result<String, String> {
    config_file_path_for(&state.config_dir)
}

/// S2: makes sure the preferences file exists before it is opened. A fresh
/// install has never written one, since defaults are served from memory until
/// the first save, so they are persisted through the settings module (which also
/// creates the config directory) instead of handing the system opener a missing
/// path. An existing file is never rewritten.
fn ensure_config_file(path: &Path) -> Result<(), String> {
    if path.is_file() {
        return Ok(());
    }
    settings::save(path, &Settings::default()).map_err(display_err)
}

/// S2: opens the preferences file in the system's default editor through the
/// opener plugin the app already installs. The path is resolved here, so the
/// webview never needs it to trigger the action.
#[tauri::command]
pub fn open_config_file(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let path = settings_path(&state);
    ensure_config_file(&path)?;
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|err| format!("cannot open {}: {err}", path.display()))
}

// ── pi runtime seam (R3) ─────────────────────────────────────────────────────

/// Directory (under the app data dir) that holds the pi-owned session files.
const SESSIONS_DIR: &str = "sessions";

/// Envelope of every event forwarded to the webview (S6). camelCase, like every
/// other payload that crosses the bridge; `sessionId` is the index row id in
/// decimal form, the same key the runtime registers the process under.
/// The TS contract (R4) mirrors this shape: `{ sessionId: string, event }`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventEnvelope {
    pub session_id: String,
    pub event: AgentEvent,
}

/// What `start_session` returns: the registered session, the pi session file it
/// owns, and the live process.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub session_id: String,
    pub workspace_id: i64,
    pub pi_session_path: String,
    pub pid: u32,
    pub status: RuntimeStatus,
}

/// What `session_status` returns. A session that is not running reports
/// `stopped` with no pid, so a poll can tell "stopped" from "no such session"
/// (the latter is an `Err`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusDto {
    pub session_id: String,
    pub status: RuntimeStatus,
    pub pid: Option<u32>,
    pub error: Option<String>,
}

/// The model one live session is really running, as pi answered `set_model`
/// (S2 of `models-by-provider.md`). pi answers with its own `Model`, so the
/// provider and the id are the ones the child resolved — an alias becomes the
/// model it names — and the panel can trust this instead of its own echo.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionModelInfo {
    /// Provider the model belongs to: model ids are provider-scoped, so a
    /// switch that answered only the id would be ambiguous.
    pub provider: String,
    pub id: String,
    /// Human label. pi always sends one; a blank falls back to the id so the
    /// selector never shows an empty name.
    pub name: String,
}

/// Stem of the session file for one start: `<workspace>-<ts>`.
/// S7: the index row is keyed by the resulting `.jsonl` path, so the stem must
/// be workspace-scoped and unique per start.
fn session_file_stem(workspace_id: i64, epoch_millis: u128) -> String {
    format!("{workspace_id}-{epoch_millis}")
}

/// Message for a startup that never reached ready. Names only what was observed
/// (the outcome and the captured stderr), never a guessed cause.
fn startup_failure_message(outcome: StartupOutcome, stderr_tail: &str) -> String {
    let reason = match outcome {
        StartupOutcome::Ready => "pi started",
        StartupOutcome::Crashed => "pi crashed before it became ready",
        StartupOutcome::Timeout => "pi did not become ready before the startup timeout",
        StartupOutcome::Aborted => "pi startup was aborted",
    };
    let tail = stderr_tail.trim();
    if tail.is_empty() {
        reason.to_string()
    } else {
        format!("{reason}: {tail}")
    }
}

/// Milliseconds since the Unix epoch, used for the session file name.
fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// ISO-8601 UTC timestamp for the session index row.
/// `store.rs` has the same helper, but it is private and this is its only other
/// caller, so the formatting is repeated here instead of widening another
/// module's API for one string.
fn now_iso() -> String {
    iso_from_system_time(std::time::SystemTime::now())
}

/// The store's `YYYY-MM-DDTHH:MM:SSZ` shape for any instant. Fixed width on
/// purpose: two of these compare correctly as plain strings, which is how the
/// preview's freshness is decided (see `preview_is_stale`).
fn iso_from_system_time(time: std::time::SystemTime) -> String {
    let secs = time
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days_since_epoch = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days_since_epoch);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Days-since-epoch to civil date (Howard Hinnant's algorithm), the conversion
/// `store.rs` uses for its timestamps.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Guards that a session the caller names is indexed in the workspace it names.
/// Every session command is scoped this way: the runtime key is the row id, and
/// a row from another workspace must never reach it.
fn ensure_session_in_workspace(
    store: &Store,
    workspace_id: i64,
    session_id: i64,
) -> Result<(), String> {
    let sessions = store.list_sessions(workspace_id).map_err(display_err)?;
    if sessions.iter().any(|session| session.id == session_id) {
        Ok(())
    } else {
        Err(format!(
            "session {session_id} is not indexed in workspace {workspace_id}"
        ))
    }
}

/// The spawn one created session still owes, with everything the worker thread
/// needs owned before the move (the closure must be `'static`, and the command
/// must not keep a borrow of the shared state).
struct SessionSpawn {
    runtime: ProcessManager,
    /// The runtime key: the index row id in decimal form.
    key: String,
    pi_binary: String,
    provider: String,
    model: String,
    /// S2 of `composer-inputs.md`: the persisted thinking level a NEW session
    /// starts at (`--thinking <level>`), or `None` when the file holds no level
    /// this build knows.
    thinking: Option<String>,
    pi_session_path: PathBuf,
    workspace_path: PathBuf,
}

impl SessionSpawn {
    /// Boots pi and waits for its readiness probe, exactly as the create command
    /// did inline before it stopped waiting.
    ///
    /// The outcome is deliberately not returned. Every transition — `spawning`,
    /// `ready`, `crashed` — and the stderr tail of a failed start leave through
    /// the runtime's own `process_status`/`process_exit` events (S4), the channel
    /// the panel already renders, so a start that fails in the background is
    /// never silent. `Err` only means the attempt could not be made at all (a
    /// live process under the same key), and that process reports for it.
    fn run(self) {
        let _ = self.runtime.spawn(
            &self.key,
            &self.pi_binary,
            &self.pi_session_path,
            SessionModel {
                provider: &self.provider,
                model: &self.model,
                // S2 of `composer-inputs.md`: the persisted level, so a NEW
                // session starts where the effort selector was left.
                thinking: self.thinking.as_deref(),
            },
            &self.workspace_path,
        );
    }
}

/// Waits for the boot-time registration before pi is spawned (S4 of
/// `webview-control.md`, defect L8).
///
/// pi reads `mcp.json` once at start, and the registration is a `remove` followed
/// by an `add`: a spawn inside that window reads a config with no
/// `nabla-browser` entry and never recovers. This is the fence in front of every
/// spawn, bounded by `REGISTRATION_WAIT` so a registration that never reports
/// costs a pause, never a session.
///
/// Synchronous, and therefore only ever called from `spawn_blocking`: a `Condvar`
/// wait on an async thread would stall the runtime that serves every other
/// command. The outcome is not discarded — a failure is announced once, with what
/// it costs the human.
fn await_mcp_registration(gate: &RegistrationGate) -> RegistrationOutcome {
    let outcome = gate.wait(REGISTRATION_WAIT);
    if let Some(notice) = gate.announce(&outcome) {
        eprintln!("{notice}");
    }
    outcome
}

/// The spawn a `start_session` owes: the gate first, then pi.
///
/// Split out of the detached task so the fence is testable without racing it: the
/// test drives this exactly as the detached task does, and can assert that pi is
/// not started while the registration is still in flight. Always called from
/// `spawn_blocking`, because `await_mcp_registration` is a blocking wait.
fn boot_session(spawn: SessionSpawn, registration: Arc<RegistrationGate>) {
    await_mcp_registration(&registration);
    spawn.run();
}

/// A `start_session` that has already answered its caller: the index row is
/// written, and the spawn pi still owes travels with it.
struct PendingSessionStart {
    /// What the command answers before pi exists: `spawning`, no pid (S1, S4).
    session: SessionDto,
    spawn: SessionSpawn,
}

/// Indexes one session for the workspace and prepares its spawn, without
/// starting pi yet (S1, S3, S7).
///
/// Everything that can fail before a child exists — the workspace lookup, the
/// settings, the session directory, the index row — happens here and keeps the
/// error mapping it always had, so a caller either gets a row it can show or the
/// same message it got before. What is left is the readiness wait, which the
/// command deliberately stops paying.
fn prepare_session_start(
    store: &Store,
    config_dir: &Path,
    data_dir: &Path,
    runtime: &ProcessManager,
    workspace_id: i64,
) -> Result<PendingSessionStart, String> {
    // A workspace is a directory on disk: spawning pi somewhere else would
    // produce a session the user never asked for.
    let workspace = workspace_by_id(store, workspace_id)?;

    // The engine path, provider and model are user-configured, so they are read
    // per start, not cached (S7).
    let settings = settings::load(&settings_file(config_dir)).map_err(display_err)?;

    let sessions_dir = data_dir.join(SESSIONS_DIR);
    std::fs::create_dir_all(&sessions_dir).map_err(display_err)?;
    let pi_session_path = sessions_dir.join(format!(
        "{}.jsonl",
        session_file_stem(workspace_id, now_millis())
    ));
    let pi_session_path_string = pi_session_path.to_string_lossy().to_string();

    // Index first: the row id is the runtime key, and no transcript is ever
    // stored here — only the path pi owns (S7).
    let timestamp = now_iso();
    let row = store
        .upsert_session(&SessionNew {
            workspace_id,
            pi_session_path: pi_session_path_string.clone(),
            title: None,
            model: None,
            started_at: timestamp.clone(),
            updated_at: timestamp,
        })
        .map_err(display_err)?;

    let session = SessionDto {
        session_id: row.id.to_string(),
        workspace_id,
        pi_session_path: pi_session_path_string,
        // Nothing is running yet: a real pid arrives with `ready`, through the
        // runtime's events, and this answer must never claim one.
        pid: 0,
        status: RuntimeStatus::Spawning,
    };
    // S2 of `composer-inputs.md`: read per start, like the pair below, so a
    // level the human changed in the composer reaches the new session. An actual
    // unknown word is dropped here instead of reaching pi's argv.
    let thinking = settings.thinking().map(str::to_string);
    let spawn = SessionSpawn {
        runtime: runtime.clone(),
        key: session.session_id.clone(),
        pi_binary: settings.pi_binary,
        provider: settings.provider,
        model: settings.model,
        thinking,
        pi_session_path,
        workspace_path: PathBuf::from(workspace.path),
    };
    Ok(PendingSessionStart { session, spawn })
}

/// Creates one headless pi for this workspace and returns its session as soon as
/// the index row exists (S1, S3, S7).
///
/// The answer is `spawning` with no pid: the readiness wait (~2 s) is no longer
/// part of the command, so a session is usable the moment it is asked for
/// instead of after pi boots. The spawn runs detached and its answer is not
/// awaited — not a silent hand-off, because the runtime reports `spawning`,
/// `ready`, `crashed` and the exit on the event bridge the panel already draws
/// (S4). `async` remains only so the task is spawned from inside the runtime,
/// never so the command waits for it.
#[tauri::command]
pub async fn start_session(
    state: State<'_, AppState>,
    workspace_id: i64,
) -> Result<SessionDto, String> {
    let PendingSessionStart { session, spawn } = prepare_session_start(
        &state.store,
        &state.config_dir,
        &state.data_dir,
        &state.runtime,
        workspace_id,
    )?;
    // Detached on purpose: nothing waits for this task and dropping its handle
    // never cancels it, so the start runs to completion either way. A failed
    // start is visible through the runtime's own events, not through this task.
    let registration = state.mcp_registration.clone();
    tauri::async_runtime::spawn(async move {
        // The same blocking spawn as before: the readiness probe is a synchronous
        // wait, so it belongs on a worker thread, not on an async one. The MCP
        // registration is waited for on that same thread and before the first
        // byte of pi's argv: pi reads `mcp.json` when it starts (L8).
        let _ = tauri::async_runtime::spawn_blocking(move || boot_session(spawn, registration))
            .await;
    });
    Ok(session)
}

/// Sends a prompt to the session's pi process. The answer that matters is the
/// forwarded event stream (S6); this only reports that pi accepted the request.
///
/// S1-S2 of `odd/tasks/composer-image-input.md`: `images` are pi's own
/// `ImageContent` payloads. They are bounded and normalized here (never trusted
/// from the webview) and the model is asked whether it accepts image input before
/// anything is sent, because a silent model switch is exactly what the feature
/// refuses.
#[tauri::command]
pub fn send_prompt(
    state: State<'_, AppState>,
    workspace_id: i64,
    session_id: i64,
    text: String,
    images: Option<Vec<PromptImage>>,
) -> Result<(), String> {
    let paths = ImageCapabilityPaths {
        config_dir: &state.config_dir,
        models: &models_path(&state),
        catalog: &catalog_path(&state),
        credentials: &credentials_path(&state),
    };
    send_prompt_for(
        &state.store,
        &paths,
        &state.runtime,
        workspace_id,
        session_id,
        text,
        images.unwrap_or_default(),
    )
}

/// S1 of `composer-image-input.md`: reads one image the human picked or dropped.
///
/// The format comes from the bytes and the file is bounded before it is read
/// (`image_input::read_image_attachment`), so this is the only path a filesystem
/// path a webview produced turns into image data.
#[tauri::command]
pub fn read_image_attachment(path: String) -> Result<ImageAttachment, String> {
    image_input::read_image_attachment(Path::new(&path)).map_err(display_err)
}

/// S3 of `composer-tools-pdf.md`: extracts one PDF the human picked or dropped,
/// locally and page by page (`pdf_input::read_pdf_attachment`).
///
/// The bounds — file size, page count, extracted characters, elapsed time — and
/// the argv-only child live in that module, so this is the only path a filesystem
/// path a webview produced becomes document text, and it answers text rather than
/// the PDF's bytes.
#[tauri::command]
pub fn read_pdf_attachment(path: String) -> Result<PdfAttachment, String> {
    crate::pdf_input::read_pdf_attachment(Path::new(&path)).map_err(display_err)
}

/// The files the image-capability preflight reads. Grouped so the helper that
/// sends a prompt keeps a readable argument list.
struct ImageCapabilityPaths<'a> {
    /// `settings.json`: the persisted provider/model a non-live session would use.
    config_dir: &'a Path,
    /// pi's `models.json`, with the models it declares.
    models: &'a Path,
    /// pi's cached catalogue, which `models.json` alone does not cover.
    catalog: &'a Path,
    /// pi's credentials: a catalogue-only provider counts only once signed in.
    credentials: &'a Path,
}

/// S2: what a model that is really running says about image input.
///
/// pi is asked (`get_state`) instead of being assumed: the model of a live session
/// may have been switched in the composer, and the persisted pair would then name
/// a different capability. `None` means "this pi does not say" — the caller falls
/// back to the catalogue — not "text only".
fn live_model_accepts_images(runtime: &ProcessManager, key: &str) -> Option<bool> {
    let status = runtime.status(key)?.status;
    if !matches!(status, RuntimeStatus::Ready | RuntimeStatus::Running) {
        return None;
    }
    let response = runtime
        .send(key, "get_state", serde_json::Map::new())
        .ok()?;
    if !response.success {
        return None;
    }
    let model = response.data.as_ref()?.get("model")?;
    image_input::model_input_has_image(model)
}

/// S2: whether the persisted pair declares image input in pi's own catalogue.
/// `None` when the catalogue does not describe that model at all, which is not
/// evidence of anything; the caller refuses instead of guessing.
fn catalogue_model_accepts_images(
    paths: &ImageCapabilityPaths<'_>,
    provider: &str,
    model_id: &str,
) -> Option<bool> {
    if provider.trim().is_empty() || model_id.trim().is_empty() {
        return None;
    }
    let providers =
        crate::providers::read_provider_models_with_catalog(paths.models, paths.catalog, paths.credentials)
            .ok()?;
    let provider_entry = providers
        .iter()
        .find(|entry| entry.provider == provider)?;
    let model = provider_entry
        .models
        .iter()
        .find(|entry| entry.id == model_id)?;
    Some(
        model
            .modalities
            .iter()
            .any(|modality| modality.eq_ignore_ascii_case("image")),
    )
}

/// The title of a session opened by an image-only turn: there is no text to
/// derive one from, and an empty drawer row would say nothing at all.
fn image_only_title(count: usize) -> String {
    match count {
        1 => "Image".to_string(),
        other => format!("{other} images"),
    }
}

/// The body of `send_prompt`, split out so the tests drive the real validation and
/// the real wire line without a Tauri `State`.
fn send_prompt_for(
    store: &Store,
    paths: &ImageCapabilityPaths<'_>,
    runtime: &ProcessManager,
    workspace_id: i64,
    session_id: i64,
    text: String,
    images: Vec<PromptImage>,
) -> Result<(), String> {
    let images = image_input::validate_prompt_images(&images).map_err(display_err)?;
    // S2: a turn needs something to say, and images are something. An image-only
    // prompt is allowed ("image-only"); an empty turn is still refused.
    if text.trim().is_empty() && images.is_empty() {
        return Err("prompt text is empty".to_string());
    }
    ensure_session_in_workspace(store, workspace_id, session_id)?;

    if !images.is_empty() {
        let settings = settings::load(&settings_file(paths.config_dir)).map_err(display_err)?;
        let supported = match live_model_accepts_images(runtime, &session_id.to_string()) {
            Some(supported) => supported,
            None => catalogue_model_accepts_images(
                paths,
                &settings.provider,
                &settings.model,
            )
            .ok_or_else(|| {
                format!(
                    "cannot confirm that model {}/{} accepts image input; pick a model that does",
                    settings.provider, settings.model
                )
            })?,
        };
        if !supported {
            return Err(format!(
                "model {}/{} does not accept image input; switch models to send images",
                settings.provider, settings.model
            ));
        }
    }

    // The first prompt names the session: the drawer shows this alias instead of
    // a generic label, and the row keeps `title = None` until then. A failure
    // here must never block the prompt, so it is deliberately ignored.
    if let Ok(Some(row)) = store.session_by_id(session_id)
        && row.title.is_none()
    {
        let title = derive_session_title(&text);
        let title = if title.is_empty() && !images.is_empty() {
            image_only_title(images.len())
        } else {
            title
        };
        let _ = store.set_session_title(session_id, &title);
    }

    let mut fields = serde_json::Map::new();
    fields.insert("message".to_string(), serde_json::Value::String(text));
    // The `images` key is absent for a text-only prompt, so that line is byte for
    // byte the one this path sent before images existed.
    if !images.is_empty() {
        fields.insert(
            "images".to_string(),
            serde_json::Value::Array(image_input::wire_images(&images)),
        );
    }
    let response = runtime
        .send(&session_id.to_string(), "prompt", fields)
        .map_err(display_err)?;
    if response.success {
        // A sent prompt is activity: the drawer sorts by `updated_at`, so the
        // session in use must climb to the top instead of sinking by creation.
        let _ = store.touch_session(session_id);
        Ok(())
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "pi refused the prompt".to_string()))
    }
}

// ── composer inputs: effort, / commands (S2, S3 of `composer-inputs.md`) ───
//
// Two proxies over one live child and one preference write. Nabla never
// interprets a command and never decides a level: pi answers which levels the
// model accepts, pi executes `/name` through the existing prompt path, and pi
// confirms the level it really adopted through `thinking_level_changed`.

/// S3: one command the session offers, as pi reports it (`get_commands`).
/// `name` never carries the leading slash — the composer adds it when it
/// completes, and the prompt path invokes it as `/name` (pi docs/rpc-commands.md).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCommand {
    pub name: String,
    /// Optional on the wire for extension commands, so an absent one stays absent
    /// instead of becoming a blank line the list would render as a subtitle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// What kind of command this is, verbatim from pi: `extension`, `prompt` or
    /// `skill`. A word from a newer pi crosses unchanged rather than being
    /// dropped, so the badge never lies about the source.
    pub source: String,
}

/// S2: the levels `get_available_thinking_levels` offers `{ "levels": [...] }`.
/// Only non-empty strings are levels; anything else is not a level pi named, so
/// the selector is offered nothing rather than something invented.
fn thinking_levels_from_response(data: Option<&serde_json::Value>) -> Vec<String> {
    data.and_then(|value| value.get("levels"))
        .and_then(serde_json::Value::as_array)
        .map(|levels| {
            levels
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|level| !level.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// S3: the commands of `get_commands`, keeping the three fields the list renders.
/// `sourceInfo` is deliberately dropped: Nabla lists and completes a command, it
/// never reads the file behind it. An entry with no name cannot be completed or
/// run, so it is skipped instead of showing a row that does nothing.
fn session_commands_from_response(data: Option<&serde_json::Value>) -> Vec<SessionCommand> {
    let Some(commands) = data
        .and_then(|value| value.get("commands"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    commands
        .iter()
        .filter_map(|command| {
            let name = command.get("name")?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            Some(SessionCommand {
                name: name.to_string(),
                // A blank description is pi saying nothing, not a subtitle.
                description: command
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string),
                source: command
                    .get("source")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect()
}

/// S2: the levels this session's model accepts, asked of the child that owns the
/// model. A session with no process is an error: an empty list would claim the
/// model supports nothing.
fn session_thinking_levels_for(
    runtime: &ProcessManager,
    key: &str,
) -> Result<Vec<String>, String> {
    let response = runtime
        .send(key, "get_available_thinking_levels", serde_json::Map::new())
        .map_err(display_err)?;
    if !response.success {
        return Err(response
            .error
            .unwrap_or_else(|| "pi refused the thinking levels query".to_string()));
    }
    Ok(thinking_levels_from_response(response.data.as_ref()))
}

/// S3: the commands the session offers. Same stance as the levels above: the
/// child is the only thing that knows, and a failed query is an error instead of
/// an empty list that reads as "this session has no commands".
fn session_commands_for(
    runtime: &ProcessManager,
    key: &str,
) -> Result<Vec<SessionCommand>, String> {
    let response = runtime
        .send(key, "get_commands", serde_json::Map::new())
        .map_err(display_err)?;
    if !response.success {
        return Err(response
            .error
            .unwrap_or_else(|| "pi refused the commands query".to_string()));
    }
    Ok(session_commands_from_response(response.data.as_ref()))
}

/// S2: applies one level to the live child and keeps it as the new default.
///
/// The live half comes first and is a no-op when the session has no process: in
/// that case the persisted default IS the change (the next spawn passes it as
/// `--thinking`), and refusing the write would make the selector useless while pi
/// boots. A child that is live but rejects the request fails the whole call and
/// leaves the file as it was — nothing is reported as applied when it was not.
///
/// The answer is what the file now holds, so the selector renders the stored word
/// instead of its own guess; the level pi really adopted still arrives as
/// `thinking_level_changed`, because pi clamps to the model.
fn set_thinking_level_for(
    config_dir: &Path,
    runtime: &ProcessManager,
    key: &str,
    level: &str,
) -> Result<String, String> {
    let level = crate::settings::parse_thinking_level(level).ok_or_else(|| {
        format!(
            "unknown thinking level: {level:?} (expected \"off\", \"minimal\", \"low\", \
             \"medium\", \"high\", \"xhigh\" or \"max\")"
        )
    })?;

    if let Some(snapshot) = runtime.status(key)
        && matches!(
            snapshot.status,
            RuntimeStatus::Ready | RuntimeStatus::Running
        )
    {
        let mut fields = serde_json::Map::new();
        fields.insert(
            "level".to_string(),
            serde_json::Value::String(level.to_string()),
        );
        let response = runtime
            .send(key, "set_thinking_level", fields)
            .map_err(display_err)?;
        if !response.success {
            return Err(response
                .error
                .unwrap_or_else(|| "pi refused the thinking level".to_string()));
        }
    }

    let path = settings_file(config_dir);
    let mut settings = settings::load(&path).map_err(display_err)?;
    settings.thinking_level = level.to_string();
    settings::save(&path, &settings).map_err(display_err)?;
    Ok(settings.thinking_level)
}

/// S2 of `models-by-provider.md`: reads the model pi named in its answer.
///
/// The response's `data` is pi's own `Model`, so its `provider`/`id`/`name` are
/// the effective ones. The requested pair stands in only when the answer names
/// none (an older or newer pi): the switch still happened, and the words the UI
/// sent are the best available label for it.
fn model_info_from_data(
    data: Option<&serde_json::Value>,
    provider: &str,
    model_id: &str,
) -> SessionModelInfo {
    let field = |key: &str| -> Option<String> {
        data?
            .get(key)?
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let id = field("id").unwrap_or_else(|| model_id.to_string());
    let name = field("name").unwrap_or_else(|| id.clone());
    SessionModelInfo {
        provider: field("provider").unwrap_or_else(|| provider.to_string()),
        id,
        name,
    }
}

/// S2 of `models-by-provider.md`: switches the model of the LIVE child.
///
/// Same plumbing as `set_thinking_level_for`: one correlated request on the
/// child's stdin (the protocol's own `set_model`, provider and model id) and the
/// same bounded wait for its answer. What differs is the stance on a session
/// with no process: there is nothing to switch, so it is an error and never a
/// quiet success. The composer keeps the pick as the next spawn's default on its
/// own (`save_settings`), which is why refusing here loses nothing.
///
/// The answer is the model pi now runs — not the words that were sent — because
/// pi re-clamps the thinking level to the new model's range and reports that
/// separately as `thinking_level_changed`.
fn set_session_model_for(
    runtime: &ProcessManager,
    key: &str,
    provider: &str,
    model_id: &str,
) -> Result<SessionModelInfo, String> {
    if provider.trim().is_empty() || model_id.trim().is_empty() {
        return Err("a model switch needs both a provider and a model id".to_string());
    }

    let status = runtime.status(key).map(|snapshot| snapshot.status);
    if !matches!(
        status,
        Some(RuntimeStatus::Ready) | Some(RuntimeStatus::Running)
    ) {
        return Err(format!(
            "session {key} is not live ({}), so its model cannot be switched",
            runtime_status_word(status)
        ));
    }

    let mut fields = serde_json::Map::new();
    fields.insert(
        "provider".to_string(),
        serde_json::Value::String(provider.to_string()),
    );
    fields.insert(
        "modelId".to_string(),
        serde_json::Value::String(model_id.to_string()),
    );
    let response = runtime
        .send(key, "set_model", fields)
        .map_err(display_err)?;
    if !response.success {
        return Err(response
            .error
            .unwrap_or_else(|| "pi refused the model switch".to_string()));
    }
    Ok(model_info_from_data(
        response.data.as_ref(),
        provider,
        model_id,
    ))
}

/// The lifecycle word of one status, for a message a human reads.
fn runtime_status_word(status: Option<RuntimeStatus>) -> &'static str {
    match status {
        Some(RuntimeStatus::Spawning) => "spawning",
        Some(RuntimeStatus::Ready) | Some(RuntimeStatus::Running) => "running",
        Some(RuntimeStatus::Stopped) => "stopped",
        Some(RuntimeStatus::Crashed) => "crashed",
        None => "no process",
    }
}

/// S2 of `models-by-provider.md`: the levels the session's model accepts, for the
/// effort selector. Proxied from `get_available_thinking_levels`, so a model
/// without reasoning support
/// answers exactly the levels pi says it has (for most, just `off`).
#[tauri::command]
pub fn session_thinking_levels(
    state: State<'_, AppState>,
    workspace_id: i64,
    session_id: i64,
) -> Result<Vec<String>, String> {
    ensure_session_in_workspace(&state.store, workspace_id, session_id)?;
    session_thinking_levels_for(&state.runtime, &session_id.to_string())
}

/// S2: the human's new effort. Applied to the live session without a respawn,
/// and persisted so the NEXT session starts there too. An unknown word is
/// rejected before anything is written or sent.
#[tauri::command]
pub fn set_session_thinking_level(
    state: State<'_, AppState>,
    workspace_id: i64,
    session_id: i64,
    level: String,
) -> Result<String, String> {
    ensure_session_in_workspace(&state.store, workspace_id, session_id)?;
    set_thinking_level_for(
        &state.config_dir,
        &state.runtime,
        &session_id.to_string(),
        &level,
    )
}

/// S2 of `models-by-provider.md`: the human's new model, applied to the LIVE
/// session without a respawn. The answer is the model pi says it now runs.
///
/// The persisted default for the next spawn is NOT written here: the composer
/// already keeps the pick in `settings.json`, and a session that is not live is
/// exactly the case where that write is the whole effect.
#[tauri::command]
pub fn set_session_model(
    state: State<'_, AppState>,
    workspace_id: i64,
    session_id: i64,
    provider: String,
    model_id: String,
) -> Result<SessionModelInfo, String> {
    ensure_session_in_workspace(&state.store, workspace_id, session_id)?;
    set_session_model_for(&state.runtime, &session_id.to_string(), &provider, &model_id)
}

/// S3: the commands this session offers (extension commands, prompt templates and
/// skills). Nabla only lists them: sending the text `/name` through the existing
/// prompt path is what runs one, and that is pi's own input expansion.
#[tauri::command]
pub fn session_commands(
    state: State<'_, AppState>,
    workspace_id: i64,
    session_id: i64,
) -> Result<Vec<SessionCommand>, String> {
    ensure_session_in_workspace(&state.store, workspace_id, session_id)?;
    session_commands_for(&state.runtime, &session_id.to_string())
}

// ── the on-demand command probe (a workspace, no session) ────────────────
//
// `/` needs a command list before any session exists, and a list kept from a
// previous launch is a snapshot that can already be wrong. Instead of storing
// one, the app now asks pi directly: a short-lived child boots, answers
// `get_commands` and is killed. Nothing is indexed and nothing is remembered, so
// the list is always this workspace's current answer.

/// Reserved prefix of the runtime key a probe runs under.
///
/// Every session's runtime key is its index row id in decimal form
/// (`session_id.to_string()`, S1 of `pi-runtime.md`), so a key that starts with
/// this prefix can never belong to a session: the two spaces are disjoint by
/// construction, not by luck. The prefix is also what tells the event sink in
/// `lib.rs` that the child is not a session, so its `process_status` frames never
/// reach the webview as an unknown session id.
const PROBE_KEY_PREFIX: &str = "probe:";

/// The runtime key one workspace's probe runs under.
fn probe_key(workspace_id: i64) -> String {
    format!("{PROBE_KEY_PREFIX}{workspace_id}")
}

/// Whether a runtime key names a probe instead of a session.
pub fn is_probe_key(key: &str) -> bool {
    key.starts_with(PROBE_KEY_PREFIX)
}

/// Name of the scratch transcript a probe is pointed at, inside the sessions
/// directory. The leading dot keeps it out of anything that lists transcripts by
/// name, and the word `probe` says plainly that it is not a session.
fn probe_file_name(workspace_id: i64) -> String {
    format!(".probe-{workspace_id}.jsonl")
}

/// The answer of one running probe, and the wait everyone else joins.
///
/// A slot lives exactly as long as its leader's [`ProbeTurn`]: the first answer
/// published wins, so a leader that published and then unwound cannot replace it.
struct ProbeSlot {
    answer: Mutex<Option<Vec<SessionCommand>>>,
    done: Condvar,
}

impl ProbeSlot {
    fn new() -> ProbeSlot {
        ProbeSlot {
            answer: Mutex::new(None),
            done: Condvar::new(),
        }
    }

    /// Publishes the first answer and wakes every waiter. A later call changes
    /// nothing — the first answer is the one every caller adopts.
    fn settle(&self, commands: Vec<SessionCommand>) {
        {
            let mut answer = lock(&self.answer);
            if answer.is_none() {
                *answer = Some(commands);
            }
        }
        self.done.notify_all();
    }

    /// Waits for the leader's answer instead of spawning a second child.
    fn wait(&self) -> Vec<SessionCommand> {
        let mut answer = lock(&self.answer);
        while answer.is_none() {
            answer = self
                .done
                .wait(answer)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        answer.clone().unwrap_or_default()
    }
}

/// What a caller found when it asked for its turn at a workspace's probe.
enum ProbeStart {
    /// Nobody was probing this workspace: this caller spawns the child, owns the
    /// cleanup and publishes its answer for whoever joins late.
    Leader(ProbeTurn),
    /// A probe was already running: no second child is spawned, and this caller
    /// waits for that one's answer instead.
    Follower(Arc<ProbeSlot>),
}

/// The leader's claim on one workspace's probe slot.
///
/// Dropping it always publishes an answer — an empty one when the leader never
/// got that far — and always releases the slot, so a waiter can never hang and a
/// later call never inherits a slot nobody will ever settle.
struct ProbeTurn {
    probes: Arc<CommandProbes>,
    workspace_id: i64,
    slot: Arc<ProbeSlot>,
}

impl ProbeTurn {
    /// Publishes the answer every waiter adopts, then releases the slot.
    fn settle(self, commands: Vec<SessionCommand>) -> Vec<SessionCommand> {
        self.slot.settle(commands.clone());
        commands
    }
}

impl Drop for ProbeTurn {
    fn drop(&mut self) {
        // A no-op once `settle` ran: only the first answer is published.
        self.slot.settle(Vec::new());
        lock(&self.probes.slots).remove(&self.workspace_id);
    }
}

/// The in-flight probes, one slot per workspace.
///
/// Held by `AppState` as an `Arc` so the leader's claim can travel with the work
/// onto a worker thread and still release the slot on the way out.
#[derive(Default)]
pub struct CommandProbes {
    slots: Mutex<HashMap<i64, Arc<ProbeSlot>>>,
}

impl CommandProbes {
    pub fn new() -> CommandProbes {
        CommandProbes::default()
    }

    /// Claims this workspace's slot, or finds the probe already running there.
    /// The check and the insert happen under one lock, so two callers can never
    /// both believe they are the leader.
    fn begin(self: &Arc<Self>, workspace_id: i64) -> ProbeStart {
        let mut slots = lock(&self.slots);
        if let Some(slot) = slots.get(&workspace_id) {
            return ProbeStart::Follower(Arc::clone(slot));
        }
        let slot = Arc::new(ProbeSlot::new());
        slots.insert(workspace_id, Arc::clone(&slot));
        ProbeStart::Leader(ProbeTurn {
            probes: Arc::clone(self),
            workspace_id,
            slot,
        })
    }
}

/// One probe, start to finish: spawn a child, ask it for its commands, stop it,
/// and remove the scratch transcript it was given.
///
/// A failure is an empty list, never an error: the composer asks this while
/// someone is typing, so the only honest answer to "pi is not installed" or "the
/// child died" is "no list right now". Nothing here writes the index — a probe is
/// not a session, and a ghost row in the sidebar would be a defect.
///
/// The boot-time MCP registration is deliberately not awaited: the probe never
/// prompts the agent, so the `nabla-browser` tools are irrelevant to a
/// `get_commands` answer, and waiting would delay the `/` list by a round trip of
/// `pi mcp` for nothing.
fn probe_workspace_commands(
    workspace: &Workspace,
    config_dir: &Path,
    data_dir: &Path,
    runtime: &ProcessManager,
) -> Vec<SessionCommand> {
    let settings = match settings::load(&settings_file(config_dir)) {
        Ok(settings) => settings,
        // A preferences file this build cannot read is the app's problem, not a
        // command list's: no list, never a dialog in front of a keystroke.
        Err(_) => return Vec::new(),
    };
    let sessions_dir = data_dir.join(SESSIONS_DIR);
    if std::fs::create_dir_all(&sessions_dir).is_err() {
        return Vec::new();
    }
    let scratch = sessions_dir.join(probe_file_name(workspace.id));
    // The file belongs to this probe alone. A previous attempt that died mid-way
    // may have left one behind, so the probe always starts from nothing.
    let _ = std::fs::remove_file(&scratch);

    let key = probe_key(workspace.id);
    // Read before the fields are moved into the spawn below.
    let thinking = settings.thinking().map(str::to_string);
    let spawn = SessionSpawn {
        runtime: runtime.clone(),
        key: key.clone(),
        pi_binary: settings.pi_binary,
        provider: settings.provider,
        model: settings.model,
        // The level is irrelevant to a command list, but it is what a session
        // start passes, and the probe is a session start in everything but bookkeeping.
        thinking,
        pi_session_path: scratch.clone(),
        workspace_path: PathBuf::from(&workspace.path),
    };
    // Runs to readiness inside `spawn`; a missing binary or a child that never
    // gets there leaves the query below with nothing to ask, which is empty.
    spawn.run();
    let answer = session_commands_for(runtime, &key);
    // The child must not outlive the question, and neither must its transcript.
    let _ = runtime.stop_and_wait(&key);
    let _ = std::fs::remove_file(&scratch);
    answer.unwrap_or_default()
}

/// One workspace's command list, behind the in-flight guard.
///
/// Concurrent calls for the same workspace share ONE outcome: the leader spawns
/// and publishes, every follower waits and returns exactly what the leader
/// answered. That is what makes a keystroke-per-request `/` list cost one child
/// per workspace, not one per character.
fn workspace_commands_core(
    probes: &Arc<CommandProbes>,
    store: &Store,
    config_dir: &Path,
    data_dir: &Path,
    runtime: &ProcessManager,
    workspace_id: i64,
) -> Result<Vec<SessionCommand>, String> {
    // An id that is not indexed is a contract violation, not a probe failure, so
    // it is reported instead of pretending the workspace has no commands. It also
    // costs no child, which is why the guard is only claimed afterwards.
    let workspace = workspace_by_id(store, workspace_id)?;
    match probes.begin(workspace_id) {
        ProbeStart::Follower(slot) => Ok(slot.wait()),
        ProbeStart::Leader(turn) => {
            let answer = probe_workspace_commands(&workspace, config_dir, data_dir, runtime);
            Ok(turn.settle(answer))
        }
    }
}

/// The commands this workspace offers, asked of a short-lived pi that exists only
/// for this question (S3 of `composer-inputs.md`, no session needed).
///
/// Nothing is persisted and no session row is created: the answer is this
/// workspace's current list, or an empty one when pi could not be asked. `async`
/// because the spawn's readiness probe is a blocking wait — it belongs on a
/// worker thread, never on the IPC thread.
#[tauri::command]
pub async fn workspace_commands(
    state: State<'_, AppState>,
    workspace_id: i64,
) -> Result<Vec<SessionCommand>, String> {
    let probes = Arc::clone(&state.probes);
    let store = state.store.clone();
    let config_dir = state.config_dir.clone();
    let data_dir = state.data_dir.clone();
    let runtime = state.runtime.clone();
    tauri::async_runtime::spawn_blocking(move || {
        workspace_commands_core(
            &probes,
            &store,
            &config_dir,
            &data_dir,
            &runtime,
            workspace_id,
        )
    })
    .await
    .map_err(|err| format!("the command probe could not run: {err}"))?
}

/// Alias for a session, derived from its first prompt: the first line with
/// whitespace collapsed, truncated on a word boundary. Deterministic and free,
/// unlike a model-written summary; a session can later be renamed by hand.
fn derive_session_title(text: &str) -> String {
    const MAX_CHARS: usize = 60;
    let first_line = text.lines().next().unwrap_or("").trim();
    let collapsed = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_CHARS {
        return collapsed;
    }
    let mut kept = String::new();
    for word in collapsed.split(' ') {
        let candidate = if kept.is_empty() {
            word.to_string()
        } else {
            format!("{kept} {word}")
        };
        if candidate.chars().count() > MAX_CHARS - 1 {
            break;
        }
        kept = candidate;
    }
    if kept.is_empty() {
        return collapsed.chars().take(MAX_CHARS - 1).collect::<String>() + "…";
    }
    format!("{kept}…")
}

#[cfg(test)]
mod title_tests {
    use super::derive_session_title;

    #[test]
    fn short_prompts_become_the_title_verbatim() {
        assert_eq!(derive_session_title("Fix the drawer"), "Fix the drawer");
    }

    #[test]
    fn only_the_first_line_counts_and_whitespace_collapses() {
        assert_eq!(
            derive_session_title("  Add   session\n\nsecond line ignored  "),
            "Add session"
        );
    }

    #[test]
    fn long_prompts_are_cut_on_a_word_boundary_with_an_ellipsis() {
        let long = "Please refactor the session lifecycle so switching does not stop any process";
        let title = derive_session_title(long);
        assert!(title.ends_with('…'), "title was {title}");
        assert!(title.chars().count() <= 60, "title was {title}");
        // The cut lands between words, never mid-word.
        let body = title.trim_end_matches('…');
        assert!(long.starts_with(body), "title was {title}");
        assert!(!body.ends_with(' '));
    }

    #[test]
    fn a_single_giant_word_is_hard_cut() {
        let title = derive_session_title(&"x".repeat(120));
        assert_eq!(title.chars().count(), 60);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn empty_prompt_yields_an_empty_title() {
        assert_eq!(derive_session_title("   \n  "), "");
    }
}

/// Answers one question the session's agent is waiting on (S3 of
/// `agent-question-bridge.md`): the answer becomes the tool's return value, so
/// the blocked turn continues with what the human chose, typed or confirmed.
///
/// Write-through by design. pi minted the request id and sends no response to an
/// answer, so there is nothing to correlate and nothing to wait for: this reports
/// only whether the line left for the child. A session that is not running is an
/// error, never a silent success — the human would otherwise believe they
/// answered.
#[tauri::command]
pub fn answer_ui_request(
    state: State<'_, AppState>,
    workspace_id: i64,
    session_id: i64,
    request_id: String,
    answer: UiAnswer,
) -> Result<(), String> {
    ensure_session_in_workspace(&state.store, workspace_id, session_id)?;
    if request_id.trim().is_empty() {
        return Err("question id is empty".to_string());
    }
    state
        .runtime
        .answer_ui_request(&session_id.to_string(), &request_id, &answer)
        .map_err(display_err)
}

/// Stops the session's pi process (S4).
///
/// S2 of `turn-continuity.md`: this asks pi to `abort` and waits for the turn to
/// settle, so the turn lands in the transcript with its own stop reason and the
/// session survives the stop. Only a child that does not settle is killed (that
/// wait and its bound live in `ProcessManager::abort_and_wait`).
#[tauri::command]
pub fn stop_session(
    state: State<'_, AppState>,
    workspace_id: i64,
    session_id: i64,
) -> Result<(), String> {
    ensure_session_in_workspace(&state.store, workspace_id, session_id)?;
    state
        .runtime
        .abort_and_wait(&session_id.to_string())
        .map(|_| ())
        .map_err(display_err)
}

/// Live status of the session's pi process (S4): what the UI polls after a crash
/// or while waiting for a restart.
#[tauri::command]
pub fn session_status(
    state: State<'_, AppState>,
    workspace_id: i64,
    session_id: i64,
) -> Result<SessionStatusDto, String> {
    ensure_session_in_workspace(&state.store, workspace_id, session_id)?;
    let key = session_id.to_string();
    match state.runtime.status(&key) {
        Some(snapshot) => Ok(SessionStatusDto {
            session_id: key,
            status: snapshot.status,
            pid: Some(snapshot.pid),
            error: snapshot.error,
        }),
        None => Ok(SessionStatusDto {
            session_id: key,
            status: RuntimeStatus::Stopped,
            pid: None,
            error: None,
        }),
    }
}

/// Directory (under the sessions dir) where a deleted session's transcript is
/// parked instead of being destroyed.
const TRASH_DIR: &str = ".trash";

/// Moves a session transcript into `<data_dir>/sessions/.trash/`, keeping a
/// unique file name, and answers where it landed.
///
/// `None` means there was nothing to archive, and it is not an error: a
/// transcript pi never wrote has no file, and a path outside the app data dir is
/// not ours to move (older rows could point anywhere), so those stay in place.
/// Only the index row is ever removed for them.
fn archive_session_file(
    data_dir: &Path,
    session_file: &Path,
) -> std::io::Result<Option<PathBuf>> {
    if !session_file.starts_with(data_dir) {
        return Ok(None);
    }
    let metadata = match std::fs::metadata(session_file) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    let Some(name) = session_file
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
    else {
        return Ok(None);
    };

    let trash = data_dir.join(SESSIONS_DIR).join(TRASH_DIR);
    std::fs::create_dir_all(&trash)?;

    // `<stem>.jsonl` first, then `<stem>.1.jsonl`, `<stem>.2.jsonl`… so two
    // same-named transcripts never overwrite the earlier archive.
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) => (stem.to_string(), format!(".{extension}")),
        None => (name.clone(), String::new()),
    };
    for counter in 0..1_000 {
        let candidate = if counter == 0 {
            name.clone()
        } else {
            format!("{stem}.{counter}{extension}")
        };
        let target = trash.join(candidate);
        if target.exists() {
            continue;
        }
        std::fs::rename(session_file, &target)?;
        return Ok(Some(target));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no free name in the session archive",
    ))
}

/// Removes a session from the app: stops its pi process when it is live, moves
/// the pi-owned transcript into the app's archive folder, then drops the index
/// row. Archiving happens before the row goes, so a delete stays recoverable and
/// a failed archive leaves the index intact instead of orphaning the file.
fn delete_session_core(
    store: &Store,
    runtime: &ProcessManager,
    data_dir: &Path,
    session_id: i64,
) -> Result<bool, String> {
    // The index row is the runtime key: an id that is not indexed is an error,
    // exactly like the other session commands.
    let row = store
        .session_by_id(session_id)
        .map_err(display_err)?
        .ok_or_else(|| unknown_session(&session_id.to_string()))?;

    // Best effort, and deliberately the hard stop: the transcript is archived and
    // the index row dropped, so the child must really be gone (S2 of
    // `turn-continuity.md` covers the human's stop, not a delete).
    // `stop_and_wait` is idempotent, so a session that was never started (or
    // already stopped) is not an error — the row still has to go.
    let _ = runtime.stop_and_wait(&session_id.to_string());

    archive_session_file(data_dir, Path::new(&row.pi_session_path)).map_err(|err| {
        format!(
            "cannot archive session file {}: {err}",
            row.pi_session_path
        )
    })?;

    store.delete_session(session_id).map_err(display_err)
}

/// Deletes a session from the app entirely (the row's ⋯ menu): the process is
/// stopped, the transcript is archived under the app data dir, and the index row
/// is removed. Returns whether a row was removed.
#[tauri::command]
pub fn delete_session(state: State<'_, AppState>, session_id: i64) -> Result<bool, String> {
    delete_session_core(&state.store, &state.runtime, &state.data_dir, session_id)
}

// ── session lifecycle seam (S1, S3, S5) ────────────────────────────────────

/// Cap on messages returned by `session_transcript`: the newest are kept, so a
/// long session still opens quickly over the bridge (S3).
const TRANSCRIPT_MESSAGE_LIMIT: usize = 400;

/// Longest text kept for one transcript entry the parser could not map to a
/// plain message; the UI shows it as a short label, never a full dump.
const TRANSCRIPT_LABEL_LIMIT: usize = 160;

/// One normalized message of a pi session file, as the webview reads it back.
/// camelCase like every other payload crossing the bridge (S3); `role` is only
/// ever `"user"`, `"assistant"` or `"tool"`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptMessage {
    pub role: String,
    pub text: String,
    pub at: Option<String>,
}

/// Message for an id that is not (or no longer) indexed, shared by every
/// session command that takes one so the wording never drifts.
fn unknown_session(session_id: &str) -> String {
    format!("unknown session: {session_id}")
}

/// Looks the session index row up by the runtime key (`sessions.id`). A
/// non-numeric id or an absent row is an error: nothing is ever spawned for a
/// row that does not exist.
fn session_row_by_id(store: &Store, session_id: &str) -> Result<SessionRow, String> {
    let id = session_id
        .parse::<i64>()
        .map_err(|_| format!("session id is not a number: {session_id}"))?;
    store
        .session_by_id(id)
        .map_err(display_err)?
        .ok_or_else(|| unknown_session(session_id))
}

/// The command payload of one live session, built from its index row plus the
/// runtime snapshot. Shared by the reuse and the spawn paths of `open_session`.
fn session_dto(row: &SessionRow, pid: u32, status: RuntimeStatus) -> SessionDto {
    SessionDto {
        session_id: row.id.to_string(),
        workspace_id: row.workspace_id,
        pi_session_path: row.pi_session_path.clone(),
        pid,
        status,
    }
}

/// S3/S5: opens the session's pi process, reusing it when it is already live.
///
/// The index row is read first, so an id that is not indexed can never reach
/// the spawn path. Then the registry decides: a live process for this key is
/// returned as-is (same pid, no second child), and only a stopped session spawns
/// pi on the session file the row points at, so pi resumes that transcript
/// instead of starting a new one. A file pi has not created yet is not an error:
/// pi creates it at exactly that path, which keeps the index and the file in
/// agreement.
///
/// `async` on purpose: the spawn waits for pi's readiness probe, which costs
/// seconds. Everything before it (the index row, the workspace, the settings)
/// is fast and stays on the caller's thread; only the spawn moves to a worker.
async fn open_session_core(
    store: &Store,
    runtime: &ProcessManager,
    config_dir: &Path,
    mcp_registration: &Arc<RegistrationGate>,
    session_id: &str,
) -> Result<SessionDto, String> {
    // The index row is the runtime key (S1): resolving it first means an id that
    // is not indexed can never reach the spawn path.
    let row = session_row_by_id(store, session_id)?;

    // S5: a live process for this key is reused untouched — its pid comes from
    // the registry, never from a second spawn. This branch never waits, so it
    // stays on the caller's thread.
    if let Some(snapshot) = runtime.status(session_id) {
        return Ok(session_dto(&row, snapshot.pid, snapshot.status));
    }

    let workspace = workspace_by_id(store, row.workspace_id)?;
    // The engine path, provider and model are user-configured, so they are read
    // per open, not cached (S7).
    let settings = settings::load(&settings_file(config_dir)).map_err(display_err)?;

    // The readiness wait is the only slow step, so it is the only one handed to
    // a worker thread: a synchronous command would hold Tauri's IPC thread for
    // the whole spawn and stall every other command meanwhile. The handle and
    // the paths are owned before the move so the closure is `'static`.
    let spawned_runtime = runtime.clone();
    let registration = mcp_registration.clone();
    let key = session_id.to_string();
    let pi_session_path = row.pi_session_path.clone();
    let workspace_path = workspace.path.clone();
    // S2 of `composer-inputs.md`: the persisted level travels into the spawn, so
    // opening a stopped session starts pi at the effort the human left behind.
    let thinking = settings.thinking();
    let spawned = tauri::async_runtime::spawn_blocking(move || {
        // pi reads `mcp.json` once at start (L8): the gate is waited for here,
        // on the worker thread, before anything is spawned.
        await_mcp_registration(&registration);
        spawned_runtime.spawn(
            &key,
            &settings.pi_binary,
            // S3: the SAME file the index points at, so pi resumes that
            // transcript instead of creating another one. A file pi has not
            // written yet is not an error: pi creates it exactly there, keeping
            // index and file equal.
            Path::new(&pi_session_path),
            SessionModel {
                provider: &settings.provider,
                model: &settings.model,
                thinking,
            },
            Path::new(&workspace_path),
        )
    })
    .await
    .map_err(display_err)?;

    let report = match spawned {
        Ok(report) => report,
        // A concurrent open registered the process between the status read and
        // this spawn: reuse it, exactly like the live branch above.
        Err(crate::process::ProcessError::AlreadyRunning(_)) => {
            return runtime
                .status(session_id)
                .map(|snapshot| session_dto(&row, snapshot.pid, snapshot.status))
                .ok_or_else(|| {
                    format!("session {session_id} reported a live process but none is registered")
                });
        }
        Err(err) => return Err(display_err(err)),
    };
    if report.outcome != StartupOutcome::Ready {
        let detail = report.stderr_tail.join("\n");
        return Err(startup_failure_message(report.outcome, &detail));
    }
    Ok(session_dto(&row, report.pid.unwrap_or(0), report.status))
}

/// S3: opens an existing session — spawns pi on its indexed file when it is
/// closed, returns the live process untouched when it is already open (S5).
///
/// `async` so the spawn's readiness wait cannot block the IPC thread; the
/// handles are cloned first, because they are what makes the shared state
/// reachable from the worker thread.
#[tauri::command]
pub async fn open_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionDto, String> {
    let store = state.store.clone();
    let runtime = state.runtime.clone();
    let config_dir = state.config_dir.clone();
    let registration = state.mcp_registration.clone();
    let opened =
        open_session_core(&store, &runtime, &config_dir, &registration, &session_id).await?;
    // Opening an existing session is activity too: it must show up as recent.
    if let Ok(row_id) = session_id.parse::<i64>() {
        let _ = store.touch_session(row_id);
    }
    Ok(opened)
}

/// S3: transcript of an existing session, read from the JSONL file pi owns.
/// The file is never copied into the index; a session pi has not written yet
/// answers with an empty transcript, not an error.
/// Roles the transcript contract exposes: the only three the webview renders.
const ROLE_USER: &str = "user";
const ROLE_ASSISTANT: &str = "assistant";
const ROLE_TOOL: &str = "tool";

/// Maps pi's role words onto the contract's three roles. Anything else is not a
/// message the UI can show, so it is skipped instead of guessed at.
fn transcript_role(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "user" => Some(ROLE_USER),
        "assistant" => Some(ROLE_ASSISTANT),
        // pi names tool turns per variant (toolResult, tool_use, toolCall...);
        // every one of them is a `tool` entry for the transcript.
        "tool" | "tooluse" | "tool_use" | "toolresult" | "tool_result" | "toolcall"
        | "tool_call" | "function" => Some(ROLE_TOOL),
        _ => None,
    }
}

/// Text of one `content` field: a plain string, or the joined texts of its
/// object parts. `None` when it yields no text at all.
fn content_text(content: &serde_json::Value) -> Option<String> {
    match content {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(parts) => {
            let pieces: Vec<&str> = parts.iter().filter_map(part_text).collect();
            if pieces.is_empty() {
                None
            } else {
                Some(pieces.join("\n"))
            }
        }
        _ => None,
    }
}

fn part_text(part: &serde_json::Value) -> Option<&str> {
    match part {
        serde_json::Value::String(text) => Some(text.as_str()),
        serde_json::Value::Object(object) => {
            object.get("text").and_then(serde_json::Value::as_str)
        }
        _ => None,
    }
}

/// First stamp the line carries, as the string the UI shows. Absent stamps stay
/// `None`; the transcript must never invent a time.
fn transcript_at(value: &serde_json::Value) -> Option<String> {
    for field in ["at", "timestamp", "ts", "time"] {
        match value.get(field) {
            Some(serde_json::Value::String(text)) => return Some(text.clone()),
            Some(serde_json::Value::Number(number)) => return Some(number.to_string()),
            _ => {}
        }
    }
    None
}

/// Bounded text so a single entry can never flood the bridge payload.
fn shortened(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    out
}

/// Label of a tool entry that carries no text: the tool's own name.
fn tool_label(message: &serde_json::Value, outer: &serde_json::Value) -> Option<String> {
    for source in [message, outer] {
        for field in ["name", "tool", "toolName", "tool_name"] {
            if let Some(text) = source.get(field).and_then(serde_json::Value::as_str) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(shortened(trimmed, TRANSCRIPT_LABEL_LIMIT));
                }
            }
        }
    }
    None
}

/// One JSONL line as a transcript message, or `None` when the line is not a
/// message this contract exposes. Deliberately tolerant: pi owns the format, so
/// only `role` + `content` of the `message` object are depended on.
fn transcript_message(value: &serde_json::Value) -> Option<TranscriptMessage> {
    let outer = value.as_object()?;
    let kind = outer
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    // Streamed partials are not stored messages: reading them would repeat the
    // same assistant text once per delta.
    if kind == "message_update" || kind == "message_delta" {
        return None;
    }
    // pi wraps a stored message as `{"type":"message","message":{..}}`; a bare
    // message object is accepted too, so an older or degraded file still reads.
    let message = match outer.get("message") {
        Some(inner) if inner.is_object() => inner,
        _ => value,
    };
    let object = message.as_object()?;
    let role = object
        .get("role")
        .and_then(serde_json::Value::as_str)
        .or_else(|| outer.get("role").and_then(serde_json::Value::as_str))
        .and_then(transcript_role)
        .or_else(|| kind.starts_with("tool").then_some(ROLE_TOOL))?;
    let mut text = object
        .get("content")
        .and_then(content_text)
        .unwrap_or_default();
    if text.trim().is_empty() && role == ROLE_TOOL {
        text = tool_label(message, value).unwrap_or_default();
    }
    if text.trim().is_empty() {
        return None;
    }
    let text = if role == ROLE_TOOL {
        shortened(&text, TRANSCRIPT_LABEL_LIMIT)
    } else {
        text
    };
    Some(TranscriptMessage {
        role: role.to_string(),
        text,
        at: transcript_at(message).or_else(|| transcript_at(value)),
    })
}

/// Normalizes a whole pi JSONL file: malformed lines and unrecognized entries
/// are dropped silently (no panic on any input), and only the newest `limit`
/// messages are kept, oldest first.
fn transcript_messages(raw: &str, limit: usize) -> Vec<TranscriptMessage> {
    let mut messages: Vec<TranscriptMessage> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|value| transcript_message(&value))
        .collect();
    if messages.len() > limit {
        messages.drain(..messages.len() - limit);
    }
    messages
}

fn session_transcript_for(
    store: &Store,
    session_id: &str,
) -> Result<Vec<TranscriptMessage>, String> {
    let row = session_row_by_id(store, session_id)?;
    let path = PathBuf::from(&row.pi_session_path);
    // pi owns the file: it is read here, never copied into the index (S1).
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        // A session pi has not written yet has no transcript, not an error.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(format!(
                "cannot read pi session file {}: {err}",
                path.display()
            ));
        }
    };
    Ok(transcript_messages(&raw, TRANSCRIPT_MESSAGE_LIMIT))
}

#[tauri::command]
pub fn session_transcript(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<TranscriptMessage>, String> {
    session_transcript_for(&state.store, &session_id)
}

// ── subagent sessions (H3 of `subagent-helpers.md`) ─────────────────────────

/// One child as the extension's manifest describes it.
///
/// Every field is optional on purpose: the manifest is written by an extension
/// this file does not compile against, so a newer or older copy of it must be
/// readable rather than fatal. A line that cannot name a session file is skipped
/// by the sync below instead of failing a whole listing.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SubagentManifestEntry {
    /// The name of this run (`review`, `review-2`), which the parent's tools use.
    name: String,
    /// The agent definition the child runs.
    agent: String,
    /// Absolute path of the child's own pi JSONL file.
    session_path: String,
    /// `queued`, `running`, `done` or `failed`.
    status: String,
    started_at: Option<String>,
    ended_at: Option<String>,
    /// The parent's own pi session file — the join key back to `sessions`.
    parent_session_path: Option<String>,
}

/// One child as the command answers it: the indexed row plus the live status the
/// manifest holds. `status` is never stored in the database, so it cannot go
/// stale between two reads of the same manifest.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentSession {
    pub session_id: i64,
    pub name: String,
    pub agent: String,
    pub session_path: String,
    pub status: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

/// Directory the extension publishes its manifests in, under the app data dir.
fn manifests_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("subagents")
}

/// Parses one manifest body: a JSON array of children. Anything else — a file a
/// crash left half-written, an object where an array belongs — contributes no
/// children and is not an error.
fn manifest_entries(body: &str) -> Vec<SubagentManifestEntry> {
    let parsed = serde_json::from_str::<serde_json::Value>(body);
    match parsed {
        Ok(serde_json::Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| serde_json::from_value(item).ok())
            .collect(),
        _ => Vec::new(),
    }
}

// How many times this thread has scanned the manifest directory.
//
// Test-only, and per thread on purpose: "one scan per query" is the property
// under test, and a process-wide counter would race with every other test that
// reads a manifest in parallel.
#[cfg(test)]
thread_local! {
    static MANIFEST_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The scan counter of this thread; see `read_manifests`.
#[cfg(test)]
fn manifest_scan_count() -> usize {
    MANIFEST_SCANS.with(|scans| scans.get())
}

/// Every child every manifest in `<data_dir>/subagents/` declares, in file order.
///
/// Read fresh on every call (H3.3): the manifest is the only place a child's live
/// status exists, and caching it here would make an open sidebar show a finished
/// child as running. It is read once per query and passed down, so one listing
/// costs one scan of the extension's directory (`subagent_sessions_for`).
fn read_manifests(data_dir: &Path) -> Vec<SubagentManifestEntry> {
    #[cfg(test)]
    MANIFEST_SCANS.with(|scans| scans.set(scans.get() + 1));
    let dir = manifests_dir(data_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        // No directory yet: no session has delegated, which is an empty answer.
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    let mut children = Vec::new();
    for file in files {
        if let Ok(body) = std::fs::read_to_string(&file) {
            children.extend(manifest_entries(&body));
        }
    }
    children
}

/// The status word the contract allows. A word this build does not know reads as
/// `unknown` — never as `done` (a run wrongly reported as finished is worse than
/// one the UI shows without a state) and never as `running` (which would look
/// live forever).
fn manifest_status(raw: &str) -> String {
    match raw.trim() {
        "queued" | "running" | "done" | "failed" => raw.trim().to_string(),
        _ => "unknown".to_string(),
    }
}

/// H3: indexes every child the manifests attribute to `parent`.
///
/// The link is the parent's own pi session file, which is the only handle the
/// extension can know about its session — a manifest that names a path no row has
/// links nothing and is skipped, because guessing the parent from "the workspace's
/// active chat" would attach a child to the wrong conversation.
///
/// `manifests` is the scan the query already did: the extension's directory is
/// read once per listing and the same snapshot decides both the rows inserted here
/// and the live status the answer carries. Reading it again in here walked every
/// manifest twice per query (`subagent_sessions_for`).
fn sync_subagent_children(
    store: &Store,
    manifests: &[SubagentManifestEntry],
    parent: &SessionRow,
) -> Result<Vec<SessionRow>, String> {
    for entry in manifests {
        if entry.parent_session_path.as_deref() != Some(parent.pi_session_path.as_str()) {
            continue;
        }
        if entry.session_path.trim().is_empty() {
            continue;
        }
        // The agent definition is what the sidebar calls the child; the run name
        // is what the parent's tools address. Without an agent, the run name is
        // all there is.
        let title = if entry.agent.trim().is_empty() {
            entry.name.clone()
        } else {
            entry.agent.clone()
        };
        let started_at = entry.started_at.clone().unwrap_or_else(now_iso);
        let updated_at = entry.ended_at.clone().unwrap_or_else(|| started_at.clone());
        store
            .ensure_child_session(&crate::store::ChildSessionNew {
                parent,
                pi_session_path: &entry.session_path,
                title: &title,
                started_at: &started_at,
                updated_at: &updated_at,
            })
            .map_err(display_err)?;
    }
    store.child_sessions(parent.id).map_err(display_err)
}

/// H3: the children of one chat, with the status the manifest holds right now.
///
/// The database contributes identity (the row, its file) and the manifest
/// contributes state, so the answer is live without a write per call: the sync
/// above only inserts a child it has never seen.
fn subagent_sessions_for(
    store: &Store,
    data_dir: &Path,
    workspace_id: i64,
    parent_session_id: i64,
) -> Result<Vec<SubagentSession>, String> {
    let parent = store
        .session_by_id(parent_session_id)
        .map_err(display_err)?
        .ok_or_else(|| unknown_session(&parent_session_id.to_string()))?;
    if parent.workspace_id != workspace_id {
        return Err(format!(
            "session {parent_session_id} is not indexed in workspace {workspace_id}"
        ));
    }
    // One scan per query: the rows are indexed from this snapshot and the status
    // of every child is read from it, so the extension's directory is never walked
    // twice for one listing.
    let manifests = read_manifests(data_dir);
    let rows = sync_subagent_children(store, &manifests, &parent)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let live = manifests
                .iter()
                .find(|entry| entry.session_path == row.pi_session_path);
            SubagentSession {
                session_id: row.id,
                name: live
                    .map(|entry| entry.name.clone())
                    .filter(|name| !name.trim().is_empty())
                    .or_else(|| row.title.clone())
                    .unwrap_or_else(|| format!("subagent-{}", row.id)),
                agent: live
                    .map(|entry| entry.agent.clone())
                    .filter(|agent| !agent.trim().is_empty())
                    .or_else(|| row.title.clone())
                    .unwrap_or_default(),
                status: live.map_or_else(|| "unknown".to_string(), |entry| {
                    manifest_status(&entry.status)
                }),
                session_path: row.pi_session_path,
                started_at: live
                    .and_then(|entry| entry.started_at.clone())
                    .or(Some(row.started_at)),
                ended_at: live.and_then(|entry| entry.ended_at.clone()),
            }
        })
        .collect())
}

/// H3.3: the children the chat at `parent_session_id` started, so the sidebar can
/// nest them under it and each one can open its own transcript (a child's JSONL is
/// a normal pi session file — `session_transcript` reads it as-is).
#[tauri::command]
pub fn subagent_sessions(
    state: State<'_, AppState>,
    workspace_id: i64,
    parent_session_id: i64,
) -> Result<Vec<SubagentSession>, String> {
    subagent_sessions_for(&state.store, &state.data_dir, workspace_id, parent_session_id)
}

// ── session preview: the instant first paint ────────────────────────────────

/// How many messages a preview keeps. Matches the tail `session-store.ts` sends,
/// and is enforced here too so a longer payload can never grow the database. It is
/// the newest messages that are kept: those are the ones a human opening a
/// conversation is looking at.
const PREVIEW_MESSAGE_LIMIT: usize = 30;

/// One message of a session preview: what the webview hands over after a turn and
/// reads back when the session is opened again.
///
/// `role` is only ever one of the three the transcript renders (any other word is
/// dropped on the way in), and `reasoning` is what the app streamed for that turn —
/// exactly the part pi's JSONL file does not store, which is why this preview exists
/// at all. camelCase like every other payload crossing the bridge.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewMessage {
    pub role: String,
    pub text: String,
    pub reasoning: Option<String>,
    pub at: Option<String>,
}

/// Freshness rule of the stored preview, and the reason the store hands
/// `updated_at` back alongside the JSON instead of judging it itself.
///
/// A snapshot is **stale** only when pi's transcript file was written at least a
/// full second *after* it. Anything else goes to the preview, and ties go to the
/// preview deliberately: the app snapshots at turn end from the same events pi used
/// to write its file, and the preview is the only of the two that carries the turn's
/// reasoning. A session with no file has nothing newer than the snapshot, and an
/// unindexed session has no file at all — neither is stale.
///
/// The timestamps are the store's fixed-width ISO shape, so they compare as strings.
fn preview_is_stale(store: &Store, session_id: i64, snapshotted_at: &str) -> bool {
    let Ok(Some(row)) = store.session_by_id(session_id) else {
        return false;
    };
    let Ok(modified) = std::fs::metadata(&row.pi_session_path).and_then(|meta| meta.modified())
    else {
        return false;
    };
    iso_from_system_time(modified).as_str() > snapshotted_at
}

/// Stores the tail of a conversation as that session's preview.
///
/// `false` means nothing was written — the session is not indexed, or the store
/// failed — and the caller carries on, because a preview is only a head start on
/// painting. Messages whose role the transcript cannot render are dropped: they
/// would cost an open instead of speeding it up.
fn save_session_preview_for(
    store: &Store,
    session_id: i64,
    messages: &[PreviewMessage],
) -> bool {
    let mut kept: Vec<PreviewMessage> = messages
        .iter()
        .filter_map(|message| {
            transcript_role(&message.role).map(|role| PreviewMessage {
                role: role.to_string(),
                text: message.text.clone(),
                reasoning: message.reasoning.clone(),
                at: message.at.clone(),
            })
        })
        .collect();
    if kept.len() > PREVIEW_MESSAGE_LIMIT {
        kept.drain(..kept.len() - PREVIEW_MESSAGE_LIMIT);
    }
    let Ok(preview) = serde_json::to_string(&kept) else {
        return false;
    };
    store
        .upsert_session_preview(session_id, &preview, &now_iso())
        .unwrap_or(false)
}

/// The stored preview of a session, or `None` when there is nothing worth
/// painting: no snapshot, one that no longer parses, or one pi's file has outgrown.
///
/// A malformed row is deliberately not an error. `None` makes the caller read pi's
/// JSONL file, exactly as it did before this preview existed, so a corrupt snapshot
/// can never keep a session from opening.
fn load_session_preview_for(store: &Store, session_id: i64) -> Option<Vec<PreviewMessage>> {
    let stored = store.session_preview(session_id).ok().flatten()?;
    if preview_is_stale(store, session_id, &stored.updated_at) {
        return None;
    }
    serde_json::from_str::<Vec<PreviewMessage>>(&stored.preview).ok()
}

/// Saves the newest turns of a session so the next open paints them before pi
/// boots (`mostrar los últimos mensajes y por detrás, abrir la sesión`).
///
/// Best-effort by contract: `false` means nothing was stored and the caller carries
/// on. pi's JSONL file is the transcript; this is a bounded copy of its tail with
/// the reasoning the file never had, so a failed save must never break the turn
/// that produced it.
#[tauri::command]
pub fn save_session_preview(
    state: State<'_, AppState>,
    session_id: i64,
    messages: Vec<PreviewMessage>,
) -> bool {
    save_session_preview_for(&state.store, session_id, &messages)
}

/// The preview of a session — its newest turns with their reasoning — or `None`
/// when the caller should read pi's file instead (see `load_session_preview_for`).
#[tauri::command]
pub fn load_session_preview(
    state: State<'_, AppState>,
    session_id: i64,
) -> Option<Vec<PreviewMessage>> {
    load_session_preview_for(&state.store, session_id)
}

/// S5: ids with a live pi process, read from the runtime registry only. The
/// list paints per-session state; it never opens anything (S2).
///
/// A command probe is a child in that registry but not a session, so its
/// reserved key is filtered out: the drawer must never paint a row for a
/// process that is about to die (see `PROBE_KEY_PREFIX`).
#[tauri::command]
pub fn live_session_ids(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(state
        .runtime
        .live_ids()
        .into_iter()
        .filter(|key| !is_probe_key(key))
        .collect())
}

// ── pi CLI bridge (S1-S4 of `pi-cli-bridge.md`) ─────────────────────────────

/// S1/S2: runs one allowlisted `pi` invocation with a mandatory deadline and
/// bounded output. The allowlist, the argv-only spawn and the kill on timeout
/// live in `cli`, so no command can bypass them.
#[tauri::command]
pub fn pi_cli_run(
    state: State<'_, AppState>,
    args: Vec<String>,
    timeout_ms: Option<u64>,
) -> Result<PiCliResult, String> {
    let settings = settings::load(&settings_path(&state)).map_err(display_err)?;
    // The binary resolution is the one the runtime already uses (S1):
    // `settings.pi_binary`, never a path invented here.
    let timeout = timeout_ms
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_millis(cli::DEFAULT_TIMEOUT_MS));
    cli::run(&args, timeout, &settings.pi_binary).map_err(display_err)
}

/// S3: starts an interactive `pi` invocation — the OAuth device-code login — and
/// answers with the session id its `pi-cli` events carry. No timeout: the human
/// decides when the flow ends.
#[tauri::command]
pub fn pi_cli_start(state: State<'_, AppState>, args: Vec<String>) -> Result<String, String> {
    let settings = settings::load(&settings_path(&state)).map_err(display_err)?;
    state
        .cli
        .start(&args, &settings.pi_binary, Arc::clone(&state.cli_sink))
        .map_err(display_err)
}

/// S3: pastes the returned device code into the live child's stdin. `false`
/// means the session is unknown or already finished, which is not an error.
#[tauri::command]
pub fn pi_cli_write(
    state: State<'_, AppState>,
    cli_session_id: String,
    text: String,
) -> Result<bool, String> {
    state.cli.write(&cli_session_id, &text).map_err(display_err)
}

/// S3: kills the live child and forgets the session. Idempotent, like `stop`.
#[tauri::command]
pub fn pi_cli_stop(state: State<'_, AppState>, cli_session_id: String) -> Result<bool, String> {
    state.cli.stop(&cli_session_id).map_err(display_err)
}

/// S4: the typed account state of one provider, read from pi's own answer.
/// Credentials stay in pi's `auth.json`: this command never sees a secret.
#[tauri::command]
pub fn pi_auth_check(state: State<'_, AppState>, provider: String) -> Result<PiAuthStatus, String> {
    let settings = settings::load(&settings_path(&state)).map_err(display_err)?;
    let result = cli::run(
        &cli::auth_check_args(&provider),
        Duration::from_millis(cli::DEFAULT_TIMEOUT_MS),
        &settings.pi_binary,
    )
    .map_err(display_err)?;
    // A probe that ran out of time says nothing about the account: report it as
    // unknown instead of inventing a login state.
    Ok(cli::auth_status_from_result(&provider, &result))
}

/// R1/S2 of `providers-pane-refinements.md`: the typed account state of many
/// providers in one IPC, probed concurrently with a bounded pool. Six providers
/// that each boot the CLI cost ~7 s one by one and two waves here.
///
/// It never fails per provider: an entry that times out, answers garbage or
/// cannot be spawned is `unknown`, so one broken provider never blanks the pane.
///
/// **Side effect (S5):** every answer is also written to the store as the last
/// known state of its provider, best effort. Writing is a cache warm-up, never a
/// precondition: a failed write is dropped and the probe still answers with what
/// it observed. The return shape is unchanged on purpose, so the UI keeps reading
/// one contract.
#[tauri::command]
pub fn pi_auth_check_many(
    state: State<'_, AppState>,
    providers: Vec<String>,
) -> Result<Vec<PiAuthStatus>, String> {
    let settings = settings::load(&settings_path(&state)).map_err(display_err)?;
    let statuses = cli::auth_check_many(
        &providers,
        Duration::from_millis(cli::DEFAULT_TIMEOUT_MS),
        &settings.pi_binary,
    );
    // After the probe, never before: the pane gets its answer even if the store
    // is busy or broken.
    persist_provider_statuses(&state.store, &statuses);
    Ok(statuses)
}

/// S5: the account state the last probe left behind, read straight from
/// `nabla.db`. Pure read: it spawns nothing, so painting the pane costs one
/// query instead of ~1.15 s per provider. The UI paints these first and lets
/// `pi_auth_check_many` correct them in the background.
#[tauri::command]
pub fn cached_provider_statuses(
    state: State<'_, AppState>,
) -> Result<Vec<ProviderStatusRow>, String> {
    cached_provider_statuses_for(&state.store)
}

/// The store read behind `cached_provider_statuses`, split out so it is testable
/// without a Tauri `State` — the same shape `session_transcript` uses.
fn cached_provider_statuses_for(store: &Store) -> Result<Vec<ProviderStatusRow>, String> {
    store.list_provider_status().map_err(display_err)
}

/// S5: stores every probe answer as the last known state of its provider.
///
/// Best effort by design: the probe answer is the product and this is a cache,
/// so a write that fails is dropped instead of failing the call. It is not
/// silent inside the store — `cached_provider_statuses` reports a broken cache to
/// whoever reads it.
fn persist_provider_statuses(store: &Store, statuses: &[PiAuthStatus]) {
    // One timestamp for the batch: every entry was answered by the same probe.
    let checked_at = now_iso();
    for status in statuses {
        let _ = store.upsert_provider_status(
            &status.provider,
            auth_state_label(status.state),
            status.credential,
            &checked_at,
        );
    }
}

/// The stored spelling of an account state: the `snake_case` names the
/// `PiAuthStatus` contract already uses, so the cached row and a live answer
/// carry the same word. Exhaustive on purpose — a new state must be named here
/// before it can be cached.
fn auth_state_label(state: cli::AuthState) -> &'static str {
    match state {
        cli::AuthState::Ready => "ready",
        cli::AuthState::NotReady => "not_ready",
        cli::AuthState::Invalid => "invalid",
        cli::AuthState::Unknown => "unknown",
    }
}

// ── MCP configuration (F0 of `tool-drawer-and-mcp.md`) ──────────────────────

/// S1: every MCP read and every MCP write is a `pi mcp` call run through the
/// allowlisted bridge. Nabla never opens `mcp.json` or `~/.pi/agent/mcp-auth.json`:
/// pi owns the schema, the merge of global and project files and the OAuth
/// tokens, so nothing here can drift from what a session sees.
///
/// The exit code is never evidence of state (L of the feature doc: `pi mcp login`
/// of an unknown server exits 0): the badges are read from `list --json`.
fn mcp_call(state: &AppState, args: &[String], timeout_ms: u64) -> Result<PiCliResult, String> {
    let settings = settings::load(&settings_path(state)).map_err(display_err)?;
    cli::run(
        args,
        Duration::from_millis(timeout_ms),
        &settings.pi_binary,
    )
    .map_err(display_err)
}

/// S1: one reading of a finished `pi mcp list --json`.
///
/// The JSON is the only evidence. A list call exits 1 when any server is
/// unhealthy — which is exactly the case the pane must paint — so a non-zero code
/// is folded into the answer instead of failing the call. A timeout is the one
/// outcome with nothing to read, and it reports an error instead of an empty list
/// that would look like "no MCP servers configured".
fn mcp_list_from(result: &PiCliResult) -> Result<Vec<McpServerSummary>, String> {
    if result.timed_out {
        return Err(format!(
            "pi mcp list did not finish within {}s",
            mcp::LIST_TIMEOUT_MS / 1000
        ));
    }
    Ok(mcp::parse_list(&result.stdout))
}

/// S1: what a mutating `pi mcp` call means. 0 is success; anything else carries
/// pi's own diagnostic on stderr, which is more useful than "it failed".
fn check_mcp_mutation(result: &PiCliResult, action: &str) -> Result<(), String> {
    match result.exit_code {
        Some(0) => Ok(()),
        Some(code) => Err(format!(
            "pi mcp {action} failed (exit {code}): {}",
            stderr_or_none(&result.stderr)
        )),
        None => Err(format!(
            "pi mcp {action} ended without an exit code: {}",
            stderr_or_none(&result.stderr)
        )),
    }
}

/// pi's diagnostic, or an explicit "none" so a failure is never reported as an
/// empty message.
fn stderr_or_none(stderr: &str) -> String {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        "no diagnostic".to_string()
    } else {
        stderr.to_string()
    }
}

/// S1: the MCP pane's listing, read from pi.
#[tauri::command]
pub fn mcp_list(state: State<'_, AppState>) -> Result<Vec<McpServerSummary>, String> {
    let result = mcp_call(&state, &mcp::list_args(), mcp::LIST_TIMEOUT_MS)?;
    mcp_list_from(&result)
}

/// S1: adds or replaces a server with the argv `mcp::add_args` builds, then
/// answers the refreshed listing so the pane never paints a stale row.
#[tauri::command]
pub fn mcp_add(
    state: State<'_, AppState>,
    draft: McpAddDraft,
) -> Result<Vec<McpServerSummary>, String> {
    let args = mcp::add_args(&draft).map_err(display_err)?;
    // Adding writes a config file, so the generic deadline is right here: it is
    // not the listing, which connects to every server.
    let result = mcp_call(&state, &args, cli::DEFAULT_TIMEOUT_MS)?;
    check_mcp_mutation(&result, "add")?;
    let listed = mcp_call(&state, &mcp::list_args(), mcp::LIST_TIMEOUT_MS)?;
    mcp_list_from(&listed)
}

/// S1: removes a server from the file named by its scope.
#[tauri::command]
pub fn mcp_remove(
    state: State<'_, AppState>,
    name: String,
    scope: McpScope,
) -> Result<Vec<McpServerSummary>, String> {
    let args = mcp::remove_args(&name, scope).map_err(display_err)?;
    let result = mcp_call(&state, &args, cli::DEFAULT_TIMEOUT_MS)?;
    check_mcp_mutation(&result, "remove")?;
    let listed = mcp_call(&state, &mcp::list_args(), mcp::LIST_TIMEOUT_MS)?;
    mcp_list_from(&listed)
}

/// S1: deletes the stored OAuth credential of one server. pi keeps the server in
/// `mcp.json`; only its sign-in is gone, so the refreshed list paints the entry
/// as needing a sign-in again.
#[tauri::command]
pub fn mcp_logout(
    state: State<'_, AppState>,
    name: String,
    scope: McpScope,
) -> Result<Vec<McpServerSummary>, String> {
    let args = mcp::logout_args(&name, scope).map_err(display_err)?;
    let result = mcp_call(&state, &args, cli::DEFAULT_TIMEOUT_MS)?;
    check_mcp_mutation(&result, "logout")?;
    let listed = mcp_call(&state, &mcp::list_args(), mcp::LIST_TIMEOUT_MS)?;
    mcp_list_from(&listed)
}

/// S1: the file a scope addresses, as an absolute path. Nothing is created: pi
/// writes the file on the first `add`, and this path only feeds the "open the
/// file" and "show me the JSON" actions.
#[tauri::command]
pub fn mcp_file_path(state: State<'_, AppState>, scope: McpScope) -> Result<String, String> {
    let project = project_root()?;
    mcp_file_path_for(&state.home_dir, &project, scope)
}

/// The project root `pi` itself resolves for `-l`: the working directory of the
/// host process. The bridge inherits it and has no cwd parameter, so this is the
/// only project a `pi mcp ... -l` call can address.
fn project_root() -> Result<PathBuf, String> {
    std::env::current_dir()
        .map_err(|err| format!("cannot resolve the project directory: {err}"))
}

/// The path resolution behind `mcp_file_path`, split out so it is testable
/// without a Tauri `State` — the same shape the settings path helpers use.
fn mcp_file_path_for(home: &Path, project: &Path, scope: McpScope) -> Result<String, String> {
    let path = mcp::file_path(home, project, scope);
    // A relative path would name a file the app cannot open from any directory.
    if !path.is_absolute() {
        return Err(format!("mcp file path is not absolute: {}", path.display()));
    }
    Ok(path.to_string_lossy().to_string())
}

/// S1: starts the OAuth sign-in of one server as an INTERACTIVE `pi mcp login`.
///
/// It deliberately reuses the bridge's interactive session (S3 of
/// `pi-cli-bridge.md`) instead of a batch call: `pi mcp login` waits at a prompt
/// for the returned redirect URL, which only exists on a terminal. The returned
/// id is the `cliSessionId` of the `pi-cli` events, so the pane consumes the same
/// stream the provider login already consumes, and `pi_cli_write` is what pastes
/// the redirect URL into the child's stdin. No timeout: the human decides when
/// the flow ends, and `pi_cli_stop` kills it.
#[tauri::command]
pub fn mcp_login_start(
    state: State<'_, AppState>,
    name: String,
    scope: McpScope,
) -> Result<String, String> {
    let settings = settings::load(&settings_path(&state)).map_err(display_err)?;
    // No `--timeout`: pi's own default (300 s) applies, so the UI never has to
    // guess how long a human needs in a browser.
    let args = mcp::login_args(&name, scope, None).map_err(display_err)?;
    state
        .cli
        .start(&args, &settings.pi_binary, Arc::clone(&state.cli_sink))
        .map_err(display_err)
}

// ── F1 of `tool-drawer-and-mcp.md`: Files and Changes ───────────────────────
//
// Every one of these is read-only and every path is confined to the workspace
// it names. The logic lives in `workspace.rs`; the commands only unfreeze the
// arguments, so what the tests cover is what the webview gets.

/// S2/S3: one directory of the workspace, for the Files tree to expand.
#[tauri::command]
pub fn fs_list_dir(workspace: String, rel_path: String) -> Result<Vec<FsEntry>, String> {
    crate::workspace::list_dir(&workspace, &rel_path)
}

/// S3: how to preview one file — decoded text, or a kind the UI renders some
/// other way. Reading here never follows a symlink out of the workspace.
#[tauri::command]
pub fn fs_read_preview(workspace: String, rel_path: String) -> Result<FsPreview, String> {
    crate::workspace::preview(&workspace, &rel_path)
}

/// S3: the bytes of one image, returned raw.
///
/// Raw bytes, not base64: `tauri::ipc::Response` carries a binary body, so the
/// webview turns it into a `Blob`/object URL without a 33 % inflation, a base64
/// dependency and a second decode. Base64 would only be simpler if the answer
/// had to travel inside JSON, and it does not.
#[tauri::command]
pub fn fs_read_image_bytes(
    workspace: String,
    rel_path: String,
) -> Result<tauri::ipc::Response, String> {
    let bytes = crate::workspace::image_bytes(&workspace, &rel_path)?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// F1: the Changes tab's list, from `git status --porcelain=v1 -z`. A folder
/// that is not a repository answers an empty list.
#[tauri::command]
pub fn git_status(workspace: String) -> Result<Vec<GitEntry>, String> {
    crate::workspace::git_status(&workspace)
}

/// F1: one file against `HEAD`, with untracked files shown as an added block and
/// the answer capped at 512 KiB.
#[tauri::command]
pub fn git_diff(workspace: String, path: String) -> Result<String, String> {
    crate::workspace::git_diff(&workspace, &path)
}

// ── F2 of `tool-drawer-and-mcp.md`: the workspace terminal ──────────────────
//
// One pty per tab, on the human's own shell, with the workspace as its working
// directory. The terminal is opt-in through `enabledTools` and is never exposed
// to the agent: pi already has its own shell tool, and a second one here would
// only widen the surface. Everything that knows how a pty lives is in `shell`;
// these commands only unfreeze the arguments.

/// The directory a terminal starts in. The workspace goes through `resolve`, the
/// same containment check the Files tab uses, so a workspace that was deleted or
/// renamed answers a readable error instead of opening a shell in `/`.
fn terminal_cwd(workspace: &str) -> Result<PathBuf, String> {
    crate::workspace::resolve(workspace, "")
}

/// S2: opens one terminal in the workspace and answers the session its
/// `shell-output` events carry.
#[tauri::command]
pub fn shell_open(state: State<'_, AppState>, workspace: String) -> Result<ShellSession, String> {
    let cwd = terminal_cwd(&workspace)?;
    // S2: the human's own shell — `$SHELL`, then zsh, then bash. Resolved here so
    // the tab can show what it actually runs.
    let shell = shell::resolve_shell();
    state
        .shells
        .open_with(
            &shell,
            &[],
            &cwd,
            shell::DEFAULT_COLS,
            shell::DEFAULT_ROWS,
            Arc::clone(&state.shell_sink),
        )
        .map_err(display_err)
}

/// S2: what the human typed, byte for byte. No newline is added here; xterm
/// sends the carriage return itself. `false` means the session is unknown or
/// already finished, which is not an error.
#[tauri::command]
pub fn shell_write(
    state: State<'_, AppState>,
    session_id: String,
    data: String,
) -> Result<bool, String> {
    state.shells.write(&session_id, &data).map_err(display_err)
}

/// S2: the panel's new size, so the shell reflows and full-screen programs get
/// their resize signal.
#[tauri::command]
pub fn shell_resize(
    state: State<'_, AppState>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<bool, String> {
    state
        .shells
        .resize(&session_id, cols, rows)
        .map_err(display_err)
}

/// S2: kills the shell and forgets the session. Idempotent, like `stop`: a tab
/// that closes twice is not an error.
#[tauri::command]
pub fn shell_close(state: State<'_, AppState>, session_id: String) -> Result<bool, String> {
    Ok(state.shells.close(&session_id))
}

// ── embedded browser: a child webview the app controls ─────────────────────
//
// A real webview, never an iframe: X-Frame-Options and CSP `frame-ancestors`
// make an iframe unusable for arbitrary sites, and an external browser is a
// different product. Each view is a child webview of the main window with its
// own id, position, size and lifetime, and `embed-nav` reports every navigation
// so the UI always knows what the pane shows. `embed.rs` documents the platform
// story: macOS first, the Webview2 caveat on Windows, and no multiwebview API at
// all on mobile.

/// One line from the webview on the dev terminal.
///
/// The webview's `console` belongs to the DevTools pane, while `pnpm tauri dev`
/// shows this process's stderr. A UI-only puzzle — which handler ran, which view
/// id was dropped — is invisible from a shell without this, and the composer and
/// the browser pane are wired in the webview.
///
/// Debug-only on both sides: the frontend wrapper is compiled out of a release
/// build (`import.meta.env.DEV`) and this prints nothing there. The caller sends
/// structure only — no prompt, no page url, no token.
#[tauri::command]
pub fn debug_log(message: String) {
    if cfg!(debug_assertions) {
        eprintln!("[nabla:ui] {message}");
    }
}

/// Creates a child webview in the main window at a logical position and size and
/// answers its id (`embed-<n>`). `url` defaults to `about:blank`.
///
/// `async` on purpose: `Window::add_child` waits on the main thread to build the
/// webview, so calling it from the main thread would stall the app, and on Windows
/// it deadlocks. The blocking build runs on a worker thread instead.
#[tauri::command]
pub async fn embed_create(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    url: Option<String>,
) -> Result<String, String> {
    let embeds = state.embeds.clone();
    tauri::async_runtime::spawn_blocking(move || embeds.create(&app, x, y, width, height, url))
        .await
        .map_err(display_err)?
        .map_err(display_err)
}

/// Moves and resizes one view, in logical units. The UI calls this with the panel
/// rect and on every resize, so the child webview covers exactly that rect.
///
/// The placement is ONE operation: position and size travel together, so the
/// platform never holds a frame that is half of the rect the panel reserves (the
/// black area a child webview paints when its frame and its layer disagree). The
/// rect is normalized on the Rust side, once (`embed::place`): whole logical
/// pixels, a size the platform accepts.
#[tauri::command]
pub fn embed_set_bounds(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    view_id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    state
        .embeds
        .set_bounds(&app, &view_id, x, y, width, height)
        .map_err(display_err)
}

/// Shows or hides one view without closing it.
///
/// The drawer is hidden with `display: none` while it stays mounted, and a child
/// webview floats ABOVE the HTML: without this call a collapsed drawer would
/// leave the page painted over the chat. Hiding is not closing — the page keeps
/// its document, its history and its JS context — so showing it again brings back
/// the same page instead of reloading it.
#[tauri::command]
pub fn embed_set_visible(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    view_id: String,
    visible: bool,
) -> Result<(), String> {
    state
        .embeds
        .set_visible(&app, &view_id, visible)
        .map_err(display_err)
}

/// Navigates one view. Only `http`, `https` and `about:blank` are accepted; a
/// `file:` or `javascript:` url is refused before the webview ever sees it.
#[tauri::command]
pub fn embed_navigate(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    view_id: String,
    url: String,
) -> Result<(), String> {
    state
        .embeds
        .navigate(&app, &view_id, &url)
        .map_err(display_err)
}

/// Evaluates a script in one view and answers the result as the JSON string the
/// platform serializes it into.
///
/// `async` plus a worker thread because the answer comes back on the main thread
/// through a callback: waiting for it there would deadlock the app.
#[tauri::command]
pub async fn embed_eval(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    view_id: String,
    script: String,
) -> Result<String, String> {
    let embeds = state.embeds.clone();
    tauri::async_runtime::spawn_blocking(move || embeds.eval(&app, &view_id, &script))
        .await
        .map_err(display_err)?
        .map_err(display_err)
}

/// Closes one view and forgets it. Idempotent: a view that is already gone
/// answers `false`.
///
/// The close is by **label**, not by registry entry: a live child webview can
/// outlive the entry that described it (the MCP driver evicts one it believes is
/// dead), and such an orphan floats above the HTML painting over the app. An
/// id-only close could never reach it — see `embed::EmbedSessions::close`.
///
/// Closing a page view closes the popup tabs it opened: they are one thing to the
/// human, and a popup left behind would be a hidden webview holding a provider's
/// live session (`drawer-responsiveness-oauth.md`, S3).
#[tauri::command]
pub fn embed_close(state: State<'_, AppState>, app: tauri::AppHandle, view_id: String) -> bool {
    state.embeds.close(&app, &view_id)
}

/// The last url the navigation handler of one view reported: the page the human
/// is looking at.
#[tauri::command]
pub fn embed_url(state: State<'_, AppState>, view_id: String) -> Result<String, String> {
    state
        .embeds
        .get(&view_id)
        .map(|view| view.url)
        .ok_or_else(|| display_err(crate::embed::EmbedError::UnknownView(view_id)))
}

/// The popups the platform has finished building, with the view that opened each
/// one (`drawer-responsiveness-oauth.md`, S3/L3).
///
/// The pane adopts a popup as a tab when `embed-popup` arrives, and asks for this
/// list when it mounts: the event can fire while the UI is not listening, and a
/// popup nobody adopted would be a hidden webview with no tab. Ids only — a popup's
/// url is a redirect url, and a redirect can carry an authorization code.
#[tauri::command]
pub fn embed_popups(state: State<'_, AppState>) -> Vec<crate::embed::PopupTab> {
    state.embeds.open_popup_tabs()
}

/// Closes every view and answers how many were closed. The app also does this on
/// exit, so no child webview outlives the window that hosts it.
#[tauri::command]
pub fn embed_close_all(state: State<'_, AppState>, app: tauri::AppHandle) -> usize {
    state.embeds.close_all(&app)
}

/// The MCP endpoint this run offers so the agent can drive those views, or `None`
/// when the port could not be bound (`webview-control.md` T1).
///
/// The token travels with it on purpose: it is what the registration of T3 writes
/// into pi's `mcp.json`, and it never leaves the machine.
#[tauri::command]
pub fn mcp_endpoint(state: State<'_, AppState>) -> Option<crate::mcp_server::Endpoint> {
    state.mcp_endpoint.clone()
}

/// S4 of `webview-control.md`: writes this run's endpoint into pi's global
/// `mcp.json`, so every session pi spawns already ships the `nabla-browser`
/// tools without the human configuring anything.
///
/// `remove` runs first, and its answer is ignored: a run that was killed (or a
/// crash) can leave an entry pointing at a dead port behind, and re-adding over it
/// is only safe when the old one is gone. On the normal first run there is
/// nothing to remove, which `pi mcp remove` reports as a failure — hence
/// best effort.
///
/// The caller treats this as best effort too: a missing `pi` binary must never
/// break boot, so the entry simply is not written and the app runs without agent
/// control of the browser.
///
/// The outcome is returned, not dropped, and the caller puts it on the
/// `RegistrationGate`: a session start must be able to tell "the entry is there"
/// from "it is not" (L8), and a failure nobody reads is how the browser tools
/// disappeared in silence once already.
pub fn register_mcp_server(state: &AppState) -> RegistrationOutcome {
    let Some(endpoint) = state.mcp_endpoint.as_ref() else {
        // Nothing was bound, so there is nothing to register.
        return RegistrationOutcome::Skipped;
    };
    match write_registration(state, endpoint) {
        Ok(()) => RegistrationOutcome::Registered,
        Err(reason) => RegistrationOutcome::Failed(reason),
    }
}

/// The write itself: `remove` then `add` through pi's CLI.
fn write_registration(
    state: &AppState,
    endpoint: &crate::mcp_server::Endpoint,
) -> Result<(), String> {
    // The argv is built before the existing entry is touched: an invalid draft
    // must not leave the human with no registration at all.
    let args =
        mcp::add_args(&crate::mcp_server::registration_draft(endpoint)).map_err(display_err)?;
    let _ = unregister_mcp_server(state);
    let result = mcp_call(state, &args, cli::DEFAULT_TIMEOUT_MS)?;
    check_mcp_mutation(&result, "add")
}

/// S4: takes the entry back out when the app exits. The url names this run's
/// ephemeral port, so what stays behind after a quit is a server that can never
/// connect again.
///
/// "No such server" is not an error: it is the state the app wants, and on a run
/// whose registration never happened it is the only state there is.
pub fn unregister_mcp_server(state: &AppState) -> Result<(), String> {
    let args = mcp::remove_args(crate::mcp_server::SERVER_NAME, McpScope::Global)
        .map_err(display_err)?;
    let result = mcp_call(state, &args, cli::DEFAULT_TIMEOUT_MS)?;
    if nothing_to_remove(&result) {
        return Ok(());
    }
    check_mcp_mutation(&result, "remove")
}

/// Whether `pi mcp remove` answered "there was nothing to remove".
///
/// pi exits 1 in that case and says `No global MCP server named "…" in <file>`, so
/// the exit code alone would report a normal first run as a failure. The phrase is
/// matched on a normalized copy: it is pi's wording, and a change in it only makes
/// this answer `false`, which the caller already treats as best effort.
fn nothing_to_remove(result: &PiCliResult) -> bool {
    result
        .stderr
        .to_ascii_lowercase()
        .contains("no global mcp server named")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Scratch directory in the system temp dir (never the worktree): one fresh
    /// Store plus a `home` folder to seed, removed in teardown.
    struct TempSeedDir(std::path::PathBuf);

    impl TempSeedDir {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "nabla_seed_{}_{}_{tag}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(path.join("home")).expect("create temp dirs");
            TempSeedDir(path)
        }

        fn home(&self) -> std::path::PathBuf {
            self.0.join("home")
        }

        fn store(&self) -> Store {
            Store::open(&self.0.join("nabla.db")).expect("open store")
        }
    }

    impl Drop for TempSeedDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn seed_default_workspace_creates_home_when_the_store_is_empty() {
        let fixture = TempSeedDir::new("empty");
        let store = fixture.store();
        assert!(store.list_workspaces().expect("list").is_empty());

        let seeded = seed_default_workspace(&store, &fixture.home()).expect("seed");

        assert!(seeded, "an empty store must be seeded");
        let all = store.list_workspaces().expect("list");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].path, fixture.home().to_string_lossy());
        assert_eq!(all[0].name, "home", "the name derives from the path");
    }

    #[test]
    fn seed_default_workspace_leaves_a_populated_store_untouched() {
        let fixture = TempSeedDir::new("populated");
        let store = fixture.store();
        store
            .add_workspace("/tmp/existing-project", None)
            .expect("add");

        let seeded = seed_default_workspace(&store, &fixture.home()).expect("seed");

        assert!(!seeded, "a populated store must not be seeded");
        let all = store.list_workspaces().expect("list");
        assert_eq!(all.len(), 1, "no extra row may be added");
        assert_eq!(all[0].path, "/tmp/existing-project");
    }

    #[test]
    fn seed_default_workspace_is_idempotent_across_launches() {
        let fixture = TempSeedDir::new("idempotent");
        let store = fixture.store();

        assert!(seed_default_workspace(&store, &fixture.home()).expect("first seed"));
        assert!(!seed_default_workspace(&store, &fixture.home()).expect("second seed"));

        let all = store.list_workspaces().expect("list");
        assert_eq!(all.len(), 1, "the UNIQUE path constraint still holds");
        assert_eq!(all[0].path, fixture.home().to_string_lossy());
    }

    // ── persisted provider status (S5 of `providers-pane-refinements.md`) ──────

    fn auth_status(provider: &str, state: cli::AuthState, credential: bool) -> PiAuthStatus {
        PiAuthStatus {
            provider: provider.to_string(),
            state,
            credential,
        }
    }

    // S5: every probe answer is stored under the contract's state spelling, with
    // the credential as a bool, so the next launch paints from the database
    // instead of booting the CLI first.
    #[test]
    fn persist_provider_statuses_records_every_probe_result() {
        let fixture = TempSeedDir::new("persist_status");
        let store = fixture.store();
        let statuses = vec![
            auth_status("openai-codex", cli::AuthState::Ready, true),
            auth_status("anthropic", cli::AuthState::NotReady, false),
            auth_status("google", cli::AuthState::Invalid, false),
            auth_status("openai", cli::AuthState::Unknown, false),
        ];

        persist_provider_statuses(&store, &statuses);

        let rows = cached_provider_statuses_for(&store).expect("read the cache");
        let providers: Vec<&str> = rows.iter().map(|row| row.provider.as_str()).collect();
        assert_eq!(
            providers,
            vec!["anthropic", "google", "openai", "openai-codex"],
            "one row per probed provider, ordered by provider id"
        );
        let row = |provider: &str| {
            rows.iter()
                .find(|row| row.provider == provider)
                .expect("a row for every probed provider")
        };
        assert_eq!(row("openai-codex").state, "ready");
        assert!(row("openai-codex").credential);
        assert_eq!(row("anthropic").state, "not_ready");
        assert!(!row("anthropic").credential);
        assert_eq!(row("google").state, "invalid");
        assert_eq!(row("openai").state, "unknown");
        for row in &rows {
            assert!(
                row.checked_at.contains('T') && row.checked_at.ends_with('Z'),
                "checked_at is the store's ISO-8601 shape, got {:?}",
                row.checked_at
            );
        }
    }

    // S5: persistence is a side effect of the probe, so a write that fails must
    // never turn a good probe answer into an error the pane has to handle.
    #[test]
    fn persist_provider_statuses_ignores_a_failed_write() {
        let fixture = TempSeedDir::new("persist_status_failure");
        let store = fixture.store();
        // Break the cache table behind the store's back: the only possible
        // outcome of the write is an error, which is exactly the best-effort path.
        {
            let conn = rusqlite::Connection::open(fixture.0.join("nabla.db")).expect("raw conn");
            conn.execute_batch("DROP TABLE provider_status;")
                .expect("drop the cache table");
        }

        persist_provider_statuses(&store, &[auth_status("openai", cli::AuthState::Ready, true)]);

        // No panic: the answer still reaches the UI, the cache just misses. The
        // read is the caller that asks for the cache and hears about it.
        assert!(
            cached_provider_statuses_for(&store).is_err(),
            "a broken cache is reported to the read, never to the probe"
        );
    }

    #[test]
    fn session_file_stem_is_workspace_scoped_and_unique_per_start() {
        assert_eq!(session_file_stem(7, 1_700_000_000_123), "7-1700000000123");
        assert_eq!(session_file_stem(7, 1_700_000_000_124), "7-1700000000124");
        assert_ne!(
            session_file_stem(7, 1_700_000_000_123),
            session_file_stem(8, 1_700_000_000_123)
        );
    }

    #[test]
    fn startup_failure_message_names_the_outcome_and_the_captured_stderr() {
        let timeout = startup_failure_message(crate::process::StartupOutcome::Timeout, "");
        assert!(timeout.contains("timeout"), "got: {timeout}");
        let crashed = startup_failure_message(crate::process::StartupOutcome::Crashed, "boom");
        assert!(crashed.contains("crash"), "got: {crashed}");
        assert!(crashed.contains("boom"), "got: {crashed}");
        let silent = startup_failure_message(crate::process::StartupOutcome::Aborted, "");
        assert!(!silent.is_empty());
    }

    // S4 of `pi-cli-bridge.md`: the account probe is exactly the documented
    // invoke, and its subcommand is one the bridge's allowlist admits.
    #[test]
    fn auth_check_args_match_the_documented_invocation() {
        assert_eq!(
            crate::cli::auth_check_args("nan"),
            vec![
                "auth".to_string(),
                "check".to_string(),
                "--provider".to_string(),
                "nan".to_string(),
                "--json".to_string(),
            ]
        );
        assert!(
            crate::cli::ALLOWED_SUBCOMMANDS.contains(&"auth"),
            "the probe can only reach pi through the allowlist"
        );
    }

    // S7: the selector lists exactly the models of pi's NaN provider, in the
    // order the provider declares them. The ids are the ones pi is spawned with.
    #[test]
    fn list_models_returns_the_eleven_nan_provider_models_in_order() {
        let models = list_models().expect("list models");
        let pairs: Vec<(&str, &str)> = models
            .iter()
            .map(|model| (model.id.as_str(), model.name.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("mimo-v2.6-flash", "MiMo V2.6 Flash (NaN)"),
                ("glm5.3-flash", "GLM 5.3 Flash (NaN)"),
                ("gemma4", "Gemma4 (NaN)"),
                ("qwen3.6", "qwen3.6 (NaN)"),
                ("qwen3.8-flash", "Qwen 3.8 Flash (NaN)"),
                ("deepseek-v4-flash", "deepseek (NaN)"),
                ("hy4", "Hy4 (NaN)"),
                ("deepseek-v4-pro", "DeepSeek V4 Pro (NaN)"),
                ("glm5.3", "GLM 5.3 (NaN)"),
                ("qwen3.8-max", "Qwen 3.8 Max (NaN)"),
                ("kimi-k3.1-pro", "Kimi K3.1 Pro (NaN)"),
            ]
        );
    }

    // S7 contract shape: `{ id, name }`, exactly two string fields.
    #[test]
    fn model_info_serializes_as_id_and_name() {
        let json = serde_json::to_value(ModelInfo {
            id: "glm5.3".to_string(),
            name: "GLM 5.3 (NaN)".to_string(),
        })
        .expect("serialize");
        assert_eq!(json["id"], serde_json::json!("glm5.3"));
        assert_eq!(json["name"], serde_json::json!("GLM 5.3 (NaN)"));
        let round_tripped: ModelInfo = serde_json::from_value(json).expect("deserialize");
        assert_eq!(round_tripped.id, "glm5.3");
        assert_eq!(round_tripped.name, "GLM 5.3 (NaN)");
    }

    // S8: the mutating workspace commands answer with the row they changed, so
    // the UI never has to guess what the update produced.
    #[test]
    fn workspace_by_id_returns_the_changed_row_and_names_unknown_ids() {
        let fixture = TempSeedDir::new("workspace_by_id");
        let store = fixture.store();
        let workspace = store
            .add_workspace("/tmp/proj-a", None)
            .expect("add workspace");

        store
            .set_workspace_pinned(workspace.id, true)
            .expect("pin");
        let pinned = workspace_by_id(&store, workspace.id).expect("pinned row");
        assert!(pinned.pinned, "the answer carries the new pin state");

        store
            .touch_workspace(workspace.id)
            .expect("touch");
        let touched = workspace_by_id(&store, workspace.id).expect("touched row");
        assert!(
            touched.last_used_at.is_some(),
            "the answer carries the new last use, got {:?}",
            touched.last_used_at
        );

        let unknown = workspace_by_id(&store, 9999);
        assert_eq!(unknown, Err("unknown workspace: 9999".to_string()));
    }

    // S2 of `settings-dialog.md`: the dialog shows the preferences path, so it
    // must be absolute and name the same file the settings commands use.
    #[test]
    fn config_file_path_is_absolute_and_names_the_settings_file() {
        let fixture = TempSeedDir::new("config_path");
        let config_dir = fixture.0.join("config");

        let resolved = config_file_path_for(&config_dir).expect("absolute config dir");

        assert!(Path::new(&resolved).is_absolute(), "got: {resolved}");
        assert!(
            resolved.ends_with("settings.json"),
            "the path must name the settings file, got: {resolved}"
        );
        // One resolver: the path shown is the path the commands read and write.
        assert_eq!(resolved, settings_file(&config_dir).to_string_lossy());
        // A relative config dir would display a path the app cannot open.
        assert!(config_file_path_for(Path::new("relative/config")).is_err());
    }

    // S2: opening the file must never hand the opener a missing path, and it
    // must never rewrite preferences the human already has.
    #[test]
    fn ensure_config_file_writes_defaults_once_and_keeps_an_existing_file() {
        let fixture = TempSeedDir::new("ensure_config");
        let path = fixture.0.join("config").join("settings.json");
        assert!(!path.exists(), "the fixture starts without a settings file");

        ensure_config_file(&path).expect("create the settings file");
        assert!(path.is_file(), "the file must exist for the opener");
        assert_eq!(settings::load(&path).expect("load"), Settings::default());

        // An existing file wins: the defaults are written once, never over it.
        let custom = Settings {
            theme: "light".to_string(),
            ..Settings::default()
        };
        settings::save(&path, &custom).expect("save custom settings");
        ensure_config_file(&path).expect("keep the existing file");
        assert_eq!(settings::load(&path).expect("load"), custom);
    }

    // ── permission modes (S1/S2 of `permission-modes.md`) ───────────────────

    /// The mode the loaded gate would read on its next tool call.
    fn projected_mode(data_dir: &Path) -> String {
        let raw = std::fs::read_to_string(settings::permissions_projection_path(data_dir))
            .expect("read the projection");
        serde_json::from_str::<serde_json::Value>(&raw).expect("projection json")["mode"]
            .as_str()
            .expect("mode is a string")
            .to_string()
    }

    // S2/L4: setting the mode persists it in `settings.json` AND rewrites the
    // projection the loaded gate reads on every tool call. The second half is
    // what makes a change reach a live session instead of the next spawn.
    #[test]
    fn set_permission_mode_persists_and_projects() {
        let fixture = SessionFixture::new("permission_mode");
        let config_dir = fixture.config_dir();
        let data_dir = fixture.path("data");
        assert_eq!(
            permission_mode_for(&config_dir).expect("read"),
            "write",
            "S6: write is the default before anything is written"
        );

        assert_eq!(
            set_permission_mode_for(&config_dir, &data_dir, "read").expect("set read"),
            "read",
            "the command answers what the file now holds"
        );
        assert_eq!(permission_mode_for(&config_dir).expect("read"), "read");
        assert_eq!(
            settings::load(&settings_file(&config_dir))
                .expect("load")
                .permission_mode,
            "read"
        );
        assert_eq!(projected_mode(&data_dir), "read");

        // The switch of a LIVE session: the very next call reads the new file.
        assert_eq!(
            set_permission_mode_for(&config_dir, &data_dir, "yolo").expect("set yolo"),
            "yolo"
        );
        assert_eq!(projected_mode(&data_dir), "yolo");
    }

    // S1/S2: a word that is not one of the three is rejected before anything is
    // written — guessing here would pick a permission level for the human — and
    // a rejected change leaves both files exactly as they were.
    #[test]
    fn set_permission_mode_rejects_an_unknown_word_and_writes_nothing() {
        let fixture = SessionFixture::new("permission_mode_bad");
        let config_dir = fixture.config_dir();
        let data_dir = fixture.path("data");
        set_permission_mode_for(&config_dir, &data_dir, "read").expect("set read");
        let settings_before = std::fs::read_to_string(settings_file(&config_dir)).expect("settings");

        for bad in ["", "lectura", "yolo!", "read mode"] {
            let err = set_permission_mode_for(&config_dir, &data_dir, bad).expect_err("reject");
            assert!(
                err.contains("unknown permission mode"),
                "{bad:?} must be rejected with a reason: {err}"
            );
        }

        assert_eq!(
            std::fs::read_to_string(settings_file(&config_dir)).expect("settings"),
            settings_before,
            "a rejected mode rewrites nothing"
        );
        assert_eq!(projected_mode(&data_dir), "read");
    }

    // L4: the projection is refreshed from the file at boot, and a settings file
    // that does not parse leaves the fail-safe `write` behind instead of an older,
    // possibly permissive projection.
    #[test]
    fn sync_permission_projection_falls_back_to_write_when_settings_are_broken() {
        let fixture = SessionFixture::new("permission_sync");
        let config_dir = fixture.config_dir();
        let data_dir = fixture.path("data");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        // A previous run left yolo behind; the file is now unreadable.
        settings::write_permission_projection(&data_dir, PermissionMode::Yolo)
            .expect("seed projection");
        std::fs::write(settings_file(&config_dir), "{ not json").expect("write broken settings");

        assert!(
            sync_permission_projection(&config_dir, &data_dir).is_err(),
            "a broken settings file is reported, not hidden"
        );
        assert_eq!(
            projected_mode(&data_dir),
            "write",
            "a broken file must never leave the agent uncaged"
        );
    }

    // L4: booting from a real file projects the mode it holds, so the first tool
    // call of the first session already obeys the human's choice.
    #[test]
    fn sync_permission_projection_reads_the_stored_mode() {
        let fixture = SessionFixture::new("permission_sync_ok");
        let config_dir = fixture.config_dir();
        let data_dir = fixture.path("data");
        settings::save(
            &settings_file(&config_dir),
            &Settings {
                permission_mode: "read".to_string(),
                ..Settings::default()
            },
        )
        .expect("save settings");

        assert_eq!(
            sync_permission_projection(&config_dir, &data_dir).expect("project"),
            PermissionMode::Read
        );
        assert_eq!(projected_mode(&data_dir), "read");
    }

    // ── providers (S1-S4 of `providers-pane.md`) ────────────────────────────

    // S3: the commands resolve pi's own file from the home directory Tauri hands
    // the state, so reading and writing never diverge.
    #[test]
    fn models_file_is_pis_own_models_json_under_the_home_dir() {
        let home = Path::new("/home/tester");
        assert_eq!(
            models_file(home),
            PathBuf::from("/home/tester/.pi/agent/models.json")
        );
    }

    // ── session lifecycle (S1, S3, S5) ──────────────────────────────────────

    /// A scripted stand-in for pi: records its argv once, answers the readiness
    /// probe, then reads stdin until it is closed. No real pi, no network.
    // The fake child of the lifecycle tests. It records its argv, its attempt
    // count and every line of its stdin, and answers the requests the composer
    // features send (`get_available_thinking_levels`, `get_commands`,
    // `set_thinking_level`, `set_model`) with pi's own documented payloads, plus a
    // generic success for anything else. `sh` on purpose: no dependency, and the tests
    // assert on real bytes rather than on a mock.
    const FAKE_SESSION_PI: &str = r#"#!/bin/sh
printf '%s\n' "$@" >> '@ARGS@'
printf 'attempt\n' >> '@ATTEMPTS@'
printf '%s\n' '{"type":"response","id":"__startup_probe__","command":"get_state","success":true,"data":{}}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> '@LINES@'
  id=`printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p'`
  case "$line" in
    *'"type":"get_available_thinking_levels"'*)
      printf '%s\n' "{\"type\":\"response\",\"id\":\"$id\",\"command\":\"get_available_thinking_levels\",\"success\":true,\"data\":{\"levels\":[\"off\",\"minimal\",\"low\",\"medium\",\"high\"]}}" ;;
    *'"type":"get_commands"'*)
      printf '%s\n' "{\"type\":\"response\",\"id\":\"$id\",\"command\":\"get_commands\",\"success\":true,\"data\":{\"commands\":[{\"name\":\"fix-tests\",\"description\":\"Fix failing tests\",\"source\":\"prompt\",\"sourceInfo\":{\"path\":\"/w/fix-tests.md\"}},{\"name\":\"skill:commit\",\"source\":\"skill\"}]}}" ;;
    *'"type":"set_thinking_level"'*)
      printf '%s\n' "{\"type\":\"response\",\"id\":\"$id\",\"command\":\"set_thinking_level\",\"success\":true}" ;;
    *'"type":"set_model"'*)
      printf '%s\n' "{\"type\":\"response\",\"id\":\"$id\",\"command\":\"set_model\",\"success\":true,\"data\":{\"provider\":\"anthropic\",\"id\":\"claude-sonnet-4-5\",\"name\":\"Claude Sonnet 4.5\",\"api\":\"anthropic-messages\",\"contextWindow\":200000}}" ;;
    *)
      printf '%s\n' "{\"type\":\"response\",\"id\":\"$id\",\"success\":true,\"data\":{}}" ;;
  esac
done
"#;

    /// A fake pi that answers the startup readiness probe and then reports its
    /// own model on `get_state`, which is what the image-capability preflight
    /// reads. `@INPUT@` is the model's `input` array as pi would send it, so one
    /// script covers a vision model and a text-only one. Every other request is
    /// accepted, and the model's prompt is recorded on stdin like any other.
    const FAKE_IMAGE_PI: &str = r#"#!/bin/sh
printf '%s\n' "$@" >> '@ARGS@'
printf 'attempt\n' >> '@ATTEMPTS@'
printf '%s\n' '{"type":"response","id":"__startup_probe__","command":"get_state","success":true,"data":{}}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> '@LINES@'
  id=`printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p'`
  case "$line" in
    *'"type":"get_state"'*)
      printf '%s\n' "{\"type\":\"response\",\"id\":\"$id\",\"command\":\"get_state\",\"success\":true,\"data\":{\"model\":{\"provider\":\"nan\",\"id\":\"vision\",\"name\":\"Vision\",\"input\":@INPUT@}}}" ;;
    *)
      printf '%s\n' "{\"type\":\"response\",\"id\":\"$id\",\"command\":\"prompt\",\"success\":true,\"data\":{\"disposition\":\"started\"}}" ;;
  esac
done
"#;

    /// A fake pi for the on-demand command probe: it records that it started and
    /// its argv, creates the session file it was told to write (what pi itself
    /// does), answers the startup probe, and answers `get_commands` after
    /// `@DELAY@` whole seconds. Creating the scratch file is what makes the
    /// "nothing survives the probe" assertions mean something.
    const FAKE_PROBE_PI: &str = r#"#!/bin/sh
printf '%s\n' "$@" >> '@ARGS@'
printf 'attempt\n' >> '@ATTEMPTS@'
session=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--session" ]; then session="$arg"; fi
  prev="$arg"
done
if [ -n "$session" ]; then : > "$session"; fi
printf '%s\n' '{"type":"response","id":"__startup_probe__","command":"get_state","success":true,"data":{}}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> '@LINES@'
  id=`printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p'`
  case "$line" in
    *'"type":"get_commands"'*)
      sleep @DELAY@
      printf '%s\n' "{\"type\":\"response\",\"id\":\"$id\",\"command\":\"get_commands\",\"success\":true,\"data\":{\"commands\":[{\"name\":\"fix-tests\",\"description\":\"Fix failing tests\",\"source\":\"prompt\",\"sourceInfo\":{\"path\":\"/w/fix-tests.md\"}},{\"name\":\"skill:commit\",\"source\":\"skill\"}]}}" ;;
    *)
      printf '%s\n' "{\"type\":\"response\",\"id\":\"$id\",\"success\":true,\"data\":{}}" ;;
  esac
done
"#;

    /// A fake pi that dies before it is ever ready: the child a probe has to
    /// survive without an error.
    const FAKE_DEAD_PI: &str = r#"#!/bin/sh
printf 'attempt\n' >> '@ATTEMPTS@'
exit 1
"#;

    /// Scratch fixture of the lifecycle tests, entirely in the system temp dir
    /// and removed on drop: the SQLite store, a config dir for settings.json,
    /// a workspace directory to spawn pi in, and the session files themselves.
    struct SessionFixture {
        dir: std::path::PathBuf,
        store: Store,
        workspace_id: i64,
    }

    impl SessionFixture {
        fn new(tag: &str) -> SessionFixture {
            let mut dir = std::env::temp_dir();
            dir.push(format!(
                "nabla_session_{}_{}_{tag}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let workspace = dir.join("workspace");
            std::fs::create_dir_all(&workspace).expect("create fixture dirs");
            let store = Store::open(&dir.join("nabla.db")).expect("open store");
            let workspace_id = store
                .add_workspace(&workspace.to_string_lossy(), None)
                .expect("add workspace")
                .id;
            SessionFixture {
                dir,
                store,
                workspace_id,
            }
        }

        fn path(&self, name: &str) -> std::path::PathBuf {
            self.dir.join(name)
        }

        fn config_dir(&self) -> std::path::PathBuf {
            self.dir.join("config")
        }

        /// Writes settings.json pointing pi at the given binary, so no test
        /// ever needs a real pi on PATH.
        fn write_settings(&self, pi_binary: &Path) {
            let settings = Settings {
                pi_binary: pi_binary.to_string_lossy().to_string(),
                ..Settings::default()
            };
            settings::save(&settings_file(&self.config_dir()), &settings).expect("save settings");
        }

        /// Indexes one session row pointing at `name` inside the fixture dir and,
        /// when asked, creates that file first: the row is the runtime key (S1).
        fn add_session(&self, name: &str, file_exists: bool, content: &str) -> SessionRow {
            let path = self.path(name);
            if file_exists {
                std::fs::write(&path, content).expect("write session file");
            }
            self.store
                .upsert_session(&SessionNew {
                    workspace_id: self.workspace_id,
                    pi_session_path: path.to_string_lossy().to_string(),
                    title: None,
                    model: None,
                    started_at: "2026-11-14T10:00:00Z".to_string(),
                    updated_at: "2026-11-14T10:00:00Z".to_string(),
                })
                .expect("index session")
        }

        #[cfg(unix)]
        fn write_fake_pi(&self, name: &str) -> std::path::PathBuf {
            let path = self.path(name);
            let body = FAKE_SESSION_PI
                .replace("@ARGS@", &path.with_extension("args").display().to_string())
                .replace("@ATTEMPTS@", &self.path("attempts").display().to_string())
                .replace("@LINES@", &path.with_extension("lines").display().to_string());
            self.write_script(name, &body)
        }

        /// A fake pi that reports a model on `get_state`: `input` is the `input`
        /// array pi sends, so `["text","image"]` and `["text"]` are the two
        /// capability answers the preflight has to tell apart.
        #[cfg(unix)]
        fn write_image_pi(&self, name: &str, input: &str) -> std::path::PathBuf {
            let path = self.path(name);
            // The model's `input` array is inside a shell double-quoted string, so
            // its own quotes are escaped before insertion.
            let input = input.replace('"', "\\\"");
            let body = FAKE_IMAGE_PI
                .replace("@ARGS@", &path.with_extension("args").display().to_string())
                .replace("@ATTEMPTS@", &self.path("attempts").display().to_string())
                .replace("@LINES@", &path.with_extension("lines").display().to_string())
                .replace("@INPUT@", &input);
            self.write_script(name, &body)
        }

        /// Writes one executable stand-in script. Every fake pi goes through here,
        /// so the permission that makes it runnable is set once.
        #[cfg(unix)]
        fn write_script(&self, name: &str, body: &str) -> std::path::PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let path = self.path(name);
            std::fs::write(&path, body).expect("write fake pi");
            let mut perms = std::fs::metadata(&path)
                .expect("stat fake pi")
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).expect("chmod fake pi");
            path
        }

        /// A fake pi for the command probe: answers `get_commands` after
        /// `delay_secs` seconds, and creates the session file it was given.
        #[cfg(unix)]
        fn write_probe_pi(&self, name: &str, delay_secs: u64) -> std::path::PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let path = self.path(name);
            let body = FAKE_PROBE_PI
                .replace("@ARGS@", &path.with_extension("args").display().to_string())
                .replace("@ATTEMPTS@", &self.path("attempts").display().to_string())
                .replace("@LINES@", &path.with_extension("lines").display().to_string())
                .replace("@DELAY@", &delay_secs.to_string());
            std::fs::write(&path, body).expect("write probe pi");
            let mut perms = std::fs::metadata(&path)
                .expect("stat probe pi")
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).expect("chmod probe pi");
            path
        }

        /// A fake pi that dies before readiness, for the degraded probe path.
        #[cfg(unix)]
        fn write_dead_pi(&self, name: &str) -> std::path::PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let path = self.path(name);
            let body = FAKE_DEAD_PI
                .replace("@ATTEMPTS@", &self.path("attempts").display().to_string());
            std::fs::write(&path, body).expect("write dead pi");
            let mut perms = std::fs::metadata(&path)
                .expect("stat dead pi")
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).expect("chmod dead pi");
            path
        }

        /// Where a probe of this fixture's workspace leaves its scratch file.
        fn probe_scratch(&self) -> std::path::PathBuf {
            self.dir
                .join(SESSIONS_DIR)
                .join(probe_file_name(self.workspace_id))
        }
    }

    impl Drop for SessionFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Runs one async core helper to completion. `open_session_core` is `async`
    /// so a spawn cannot block the IPC thread; what the tests assert about it is
    /// unchanged, so they only need a way to wait for it.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tauri::async_runtime::block_on(future)
    }

    /// Short timeouts so lifecycle tests never wait out real minutes.
    fn lifecycle_timeouts() -> crate::process::Timeouts {
        crate::process::Timeouts {
            ready: Duration::from_secs(5),
            probe_interval: Duration::from_millis(50),
            stop_grace: Duration::from_millis(800),
            kill_grace: Duration::from_millis(400),
            response: Duration::from_secs(5),
            abort_grace: Duration::from_millis(600),
        }
    }

    /// A manager with a sink that throws events away: these tests assert on the
    /// registry and the argv the child actually received.
    fn lifecycle_runtime() -> ProcessManager {
        let sink: crate::process::EventSink = Arc::new(|_, _| {});
        ProcessManager::with_timeouts(sink, lifecycle_timeouts())
    }

    /// Every complete line the fake child received on stdin, bounded by a
    /// deadline because it is written by the child's own process.
    #[cfg(unix)]
    fn recorded_stdin(fixture: &SessionFixture, name: &str) -> Vec<String> {
        let path = fixture.path(name).with_extension("lines");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(raw) = std::fs::read_to_string(&path) {
                let lines: Vec<String> = raw.lines().map(|line| line.to_string()).collect();
                // The startup probe is the first line; a real request is what the
                // caller is waiting for.
                if lines.iter().any(|line| !line.contains("__startup_probe__")) {
                    return lines;
                }
            }
            assert!(
                Instant::now() < deadline,
                "fake pi never received a request at {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A gate that already reported success: the lifecycle tests are about the
    /// spawn, not about waiting for the boot-time registration.
    fn registered_gate() -> Arc<RegistrationGate> {
        let gate = Arc::new(RegistrationGate::new());
        gate.complete(RegistrationOutcome::Registered);
        gate
    }

    fn process_attempts(fixture: &SessionFixture) -> usize {
        std::fs::read_to_string(fixture.path("attempts"))
            .map(|raw| raw.lines().count())
            .unwrap_or(0)
    }

    /// Arguments the fake pi recorded, waiting briefly because the writer is the
    /// child process, not the test thread.
    #[cfg(unix)]
    fn recorded_argv(fixture: &SessionFixture, script_name: &str) -> Vec<String> {
        let log = fixture.path(script_name).with_extension("args");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(raw) = std::fs::read_to_string(&log)
                && !raw.trim().is_empty()
            {
                return raw.lines().map(|line| line.to_string()).collect();
            }
            assert!(
                Instant::now() < deadline,
                "fake pi never recorded its arguments at {}",
                log.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    // S5: opening an already-open session reuses its process. One spawn attempt
    // proves no second child was started, and the pid must be the same one: the
    // row id is the runtime key (S1).
    #[cfg(unix)]
    #[test]
    fn open_session_twice_reuses_the_same_process() {
        let fixture = SessionFixture::new("open_idempotent");
        let script = fixture.write_fake_pi("fake-pi.sh");
        fixture.write_settings(&script);
        let row = fixture.add_session("s1.jsonl", true, "");
        let runtime = lifecycle_runtime();
        let key = row.id.to_string();
        let gate = registered_gate();

        let first = block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &gate,
            &key,
        ))
        .expect("first open");
        assert_eq!(first.status, RuntimeStatus::Ready);
        assert!(first.pid > 0);
        assert_eq!(
            first.pi_session_path, row.pi_session_path,
            "the answer carries the indexed session file"
        );
        assert_eq!(
            runtime.live_ids(),
            vec![key.clone()],
            "the id is the live runtime key"
        );

        let second = block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &gate,
            &key,
        ))
        .expect("second open");
        assert_eq!(second.pid, first.pid, "S5: no second process per session");
        assert_eq!(second.status, first.status);
        assert_eq!(
            process_attempts(&fixture),
            1,
            "an already-open session never respawns"
        );

        runtime.stop_all();
    }

    // S3: a closed session opens on the file the index points at, so pi resumes
    // that transcript instead of starting a new one.
    #[cfg(unix)]
    #[test]
    fn open_session_spawns_on_the_indexed_session_file() {
        let fixture = SessionFixture::new("open_session_file");
        let script = fixture.write_fake_pi("fake-pi.sh");
        fixture.write_settings(&script);
        let row = fixture.add_session("s7.jsonl", true, "");
        let runtime = lifecycle_runtime();

        let dto = block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &registered_gate(),
            &row.id.to_string(),
        ))
        .expect("open session");
        assert_eq!(dto.status, RuntimeStatus::Ready);
        assert_eq!(dto.session_id, row.id.to_string());

        let argv = recorded_argv(&fixture, "fake-pi.sh");
        let session_at = argv
            .iter()
            .position(|arg| arg == "--session")
            .expect("the child must be told which session to resume");
        assert_eq!(
            argv.get(session_at + 1).map(String::as_str),
            Some(row.pi_session_path.as_str()),
            "S3: pi opens the indexed file, got {argv:?}"
        );
        // The whole invocation stays the existing shape: rpc mode plus the
        // configured provider/model (S7).
        assert_eq!(argv.first().map(String::as_str), Some("--mode"));
        assert_eq!(argv.get(1).map(String::as_str), Some("rpc"));

        runtime.stop_all();
    }

    // Defect L8: the wait for the boot-time registration is a synchronous
    // `Condvar` wait, so it runs on a `spawn_blocking` worker, never on an async
    // thread. This test runs the reopen path on a *current-thread* runtime: an
    // inline wait would hold that single thread for the whole timeout, so the
    // completion below could not run until it had expired.
    #[cfg(unix)]
    #[test]
    fn the_registration_wait_never_blocks_the_async_runtime() {
        let fixture = SessionFixture::new("registration_async");
        let script = fixture.write_fake_pi("fake-pi.sh");
        fixture.write_settings(&script);
        let row = fixture.add_session("s9.jsonl", true, "");
        let runtime = lifecycle_runtime();
        let gate = Arc::new(RegistrationGate::new());

        let store = fixture.store.clone();
        let manager = runtime.clone();
        let config_dir = fixture.config_dir();
        let key = row.id.to_string();
        let waiting = gate.clone();
        let current_thread = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime");

        let started = Instant::now();
        let dto = current_thread
            .block_on(async move {
                let open = tokio::spawn(async move {
                    open_session_core(&store, &manager, &config_dir, &waiting, &key).await
                });
                // The task is what reaches the wait: yield so it gets there, then
                // report from this same thread. An inline wait would keep these
                // lines from running at all until the timeout had passed.
                tokio::task::yield_now().await;
                std::thread::sleep(Duration::from_millis(80));
                gate.complete(RegistrationOutcome::Registered);
                open.await.expect("the open task must not panic")
            })
            .expect("a session opens while the registration is still running");
        let elapsed = started.elapsed();

        assert_eq!(dto.status, RuntimeStatus::Ready);
        assert!(
            elapsed < Duration::from_secs(3),
            "the wait is off the async thread: the session opened after {elapsed:?}"
        );
        runtime.stop_all();
    }

    // The defect itself (L8), on the path it was reported from: a session created
    // right after boot must not start pi while the `nabla-browser` entry is still
    // being written, because pi reads `mcp.json` exactly once.
    #[cfg(unix)]
    #[test]
    fn a_session_created_while_the_registration_runs_waits_before_spawning() {
        let fixture = SessionFixture::new("start_gate");
        let script = fixture.write_fake_pi("fake-pi.sh");
        fixture.write_settings(&script);
        let runtime = lifecycle_runtime();
        let gate = Arc::new(RegistrationGate::new());

        let pending = prepare_session_start(
            &fixture.store,
            &fixture.config_dir(),
            &fixture.dir,
            &runtime,
            fixture.workspace_id,
        )
        .expect("prepare a start");
        let key = pending.session.session_id.clone();

        // Exactly what the detached task runs, on a worker thread of its own.
        let booting = {
            let gate = gate.clone();
            std::thread::spawn(move || boot_session(pending.spawn, gate))
        };

        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            process_attempts(&fixture),
            0,
            "pi must not start inside the registration window"
        );

        gate.complete(RegistrationOutcome::Registered);
        booting.join().expect("the boot thread panicked");

        assert_eq!(process_attempts(&fixture), 1, "pi starts once the entry is there");
        assert_eq!(
            runtime.live_ids(),
            vec![key],
            "the index row id is still the runtime key"
        );

        runtime.stop_all();
    }

    // The honesty half of L8: a failed registration costs the browser tools, not
    // the session. The pane and the app work without them, so a session that
    // cannot have the tools still starts — and says so once, with its cost.
    #[cfg(unix)]
    #[test]
    fn a_failed_registration_still_lets_the_session_start() {
        let fixture = SessionFixture::new("registration_failed");
        let script = fixture.write_fake_pi("fake-pi.sh");
        fixture.write_settings(&script);
        let row = fixture.add_session("s10.jsonl", true, "");
        let runtime = lifecycle_runtime();
        let gate = Arc::new(RegistrationGate::new());
        gate.complete(RegistrationOutcome::Failed("pi is not installed".to_string()));

        let dto = block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &gate,
            &row.id.to_string(),
        ))
        .expect("a failed registration must not block the session");
        assert_eq!(dto.status, RuntimeStatus::Ready);
        assert_eq!(process_attempts(&fixture), 1, "pi still starts");

        runtime.stop_all();
    }

    // S3 negative guard: an id that is not indexed is an error and must never
    // start a process.
    #[cfg(unix)]
    #[test]
    fn open_session_for_an_unknown_id_errors_without_spawning() {
        let fixture = SessionFixture::new("open_unknown");
        let script = fixture.write_fake_pi("fake-pi.sh");
        fixture.write_settings(&script);
        let runtime = lifecycle_runtime();

        let err = block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &registered_gate(),
            "424242",
        ))
        .expect_err("an unindexed id cannot be opened");
        assert!(
            err.contains("unknown session") && err.contains("424242"),
            "the error names the id, got {err:?}"
        );
        // A non-numeric id is rejected before anything is spawned too.
        let non_numeric = block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &registered_gate(),
            "not-a-number",
        ))
        .expect_err("a non-numeric id cannot be opened");
        assert!(non_numeric.contains("not-a-number"), "got {non_numeric:?}");

        assert!(runtime.live_ids().is_empty(), "no process may be created");
        assert_eq!(
            process_attempts(&fixture),
            0,
            "the engine must never be started for an absent row"
        );
    }

    // ── creating a session without waiting for pi ──────────────────────────

    /// Records the lifecycle transitions a spawn reports, so a test can assert
    /// what the webview would have received through the event bridge.
    fn recording_runtime() -> (ProcessManager, Arc<Mutex<Vec<RuntimeStatus>>>) {
        let seen: Arc<Mutex<Vec<RuntimeStatus>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        let sink: crate::process::EventSink = Arc::new(move |_, event| {
            if let AgentEvent::ProcessStatus { status, .. } = event {
                recorded.lock().expect("record status").push(*status);
            }
        });
        (
            ProcessManager::with_timeouts(sink, lifecycle_timeouts()),
            seen,
        )
    }

    // S1/S7: creating a session answers as soon as its index row exists. The
    // engine boots behind that answer, so the caller gets `spawning` with no pid
    // instead of waiting out pi's readiness probe — the wait that kept the
    // composer disabled for seconds.
    #[cfg(unix)]
    #[test]
    fn start_session_answers_spawning_before_pi_boots() {
        let fixture = SessionFixture::new("start_spawning");
        let script = fixture.write_fake_pi("fake-pi.sh");
        fixture.write_settings(&script);
        let runtime = lifecycle_runtime();

        let pending = prepare_session_start(
            &fixture.store,
            &fixture.config_dir(),
            &fixture.dir,
            &runtime,
            fixture.workspace_id,
        )
        .expect("prepare a start");

        assert_eq!(pending.session.status, RuntimeStatus::Spawning);
        assert_eq!(pending.session.pid, 0, "nothing is running yet");
        assert_eq!(pending.session.workspace_id, fixture.workspace_id);

        // The row is already readable through the index: the panel renders the
        // session before pi exists.
        let indexed = fixture
            .store
            .list_sessions(fixture.workspace_id)
            .expect("list sessions");
        assert_eq!(indexed.len(), 1, "the index row is written before the spawn");
        assert_eq!(indexed[0].id.to_string(), pending.session.session_id);
        assert_eq!(indexed[0].pi_session_path, pending.session.pi_session_path);

        // The path pi owns lives in the app's sessions directory and nothing has
        // written it yet: pi creates it.
        let session_file = PathBuf::from(&pending.session.pi_session_path);
        assert!(
            session_file.starts_with(fixture.dir.join("sessions")),
            "got {}",
            session_file.display()
        );
        assert!(!session_file.exists(), "pi owns that file, not the app");

        // Preparing is not starting: no child, not even an attempt.
        assert!(runtime.live_ids().is_empty());
        assert_eq!(process_attempts(&fixture), 0);
    }

    // The other half of that answer: the background spawn is not a silent one.
    // Every transition reaches the sink the host forwards to the webview, so a
    // start that ends `ready` — or `crashed` — is visible there and nowhere else.
    #[cfg(unix)]
    #[test]
    fn the_background_spawn_reports_its_lifecycle_through_the_runtime_events() {
        let fixture = SessionFixture::new("start_spawn_events");
        let script = fixture.write_fake_pi("fake-pi.sh");
        fixture.write_settings(&script);
        let (runtime, seen) = recording_runtime();

        let pending = prepare_session_start(
            &fixture.store,
            &fixture.config_dir(),
            &fixture.dir,
            &runtime,
            fixture.workspace_id,
        )
        .expect("prepare a start");
        // Exactly what the detached task runs; run here so the test can assert
        // on the events instead of racing them.
        pending.spawn.run();

        let statuses = seen.lock().expect("read statuses").clone();
        assert_eq!(
            statuses.first(),
            Some(&RuntimeStatus::Spawning),
            "the attempt is announced before anything can fail: {statuses:?}"
        );
        assert!(
            statuses.contains(&RuntimeStatus::Ready),
            "a start that lands is reported as ready: {statuses:?}"
        );
        assert_eq!(process_attempts(&fixture), 1, "one child, one attempt");
        assert_eq!(
            runtime.live_ids(),
            vec![pending.session.session_id.clone()],
            "the index row id is the runtime key"
        );

        runtime.stop_all();
    }

    // Everything that can fail before a child exists keeps the error mapping it
    // always had, and nothing is left behind when it does fail.
    #[test]
    fn start_session_keeps_the_error_map_of_the_steps_before_the_spawn() {
        let fixture = SessionFixture::new("start_errors");
        let runtime = lifecycle_runtime();

        let unknown = prepare_session_start(
            &fixture.store,
            &fixture.config_dir(),
            &fixture.dir,
            &runtime,
            4242,
        )
        .err()
        .expect("an unindexed workspace cannot start a session");
        assert_eq!(unknown, "unknown workspace: 4242");

        // Unreadable preferences are the same failure they were before: pi's
        // path, provider and model are configured per start.
        std::fs::create_dir_all(fixture.config_dir()).expect("create config dir");
        std::fs::write(settings_file(&fixture.config_dir()), "{not json at all")
            .expect("write broken settings");
        let broken = prepare_session_start(
            &fixture.store,
            &fixture.config_dir(),
            &fixture.dir,
            &runtime,
            fixture.workspace_id,
        )
        .err()
        .expect("unreadable settings cannot start a session");
        assert!(!broken.is_empty(), "the failure carries a reason");

        // A failed prepare leaves no row behind, so the index never lists a
        // session no process could ever be started for.
        assert!(
            fixture
                .store
                .list_sessions(fixture.workspace_id)
                .expect("list sessions")
                .is_empty(),
            "nothing is indexed when the start failed before the spawn"
        );
        assert!(runtime.live_ids().is_empty());
    }

    // S3: the transcript is normalized from pi's own file. The known shapes are
    // a text message (content as parts), a tool entry, and plenty of lines this
    // parser must ignore instead of failing on.
    #[test]
    fn session_transcript_normalizes_known_lines_and_skips_garbage() {
        let fixture = SessionFixture::new("transcript_shapes");
        let raw = concat!(
            r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"hola"}],"timestamp":"2026-11-14T10:00:00Z"}}"#,
            "\n",
            r#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"parte uno"},{"type":"text","text":"parte dos"}]}}"#,
            "\n",
            r#"{"type":"message","message":{"role":"user","content":"texto plano"}}"#,
            "\n",
            r#"{"type":"tool_result","toolName":"bash","content":[{"type":"text","text":"salida del tool"}]}"#,
            "\n",
            "this line is not json at all",
            "\n",
            r#"{"type":"message","message":{"role":"system","content":"ignored"}}"#,
            "\n",
            r#"{"type":"message","message":{"role":"assistant","content":[]}}"#,
            "\n",
            "",
        );
        let row = fixture.add_session("s1.jsonl", true, raw);

        let messages = session_transcript_for(&fixture.store, &row.id.to_string())
            .expect("transcript");
        assert_eq!(messages.len(), 4, "garbage, unknown roles and empty text are skipped");
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].text, "hola");
        assert_eq!(messages[0].at.as_deref(), Some("2026-11-14T10:00:00Z"));
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(
            messages[1].text, "parte uno\nparte dos",
            "array content is the text of its parts"
        );
        assert_eq!(messages[1].at, None, "a line without a stamp has no `at`");
        assert_eq!(messages[2].role, "user");
        assert_eq!(messages[2].text, "texto plano", "string content is accepted");
        assert_eq!(messages[3].role, "tool");
        assert_eq!(messages[3].text, "salida del tool");

        // The roles are a closed set, whatever the file says.
        for message in &messages {
            assert!(
                ["user", "assistant", "tool"].contains(&message.role.as_str()),
                "unexpected role: {}",
                message.role
            );
        }
    }

    // S3: a session pi has not written yet opens with an empty transcript, and
    // an unindexed id is still an error (never an empty silent success).
    #[test]
    fn session_transcript_is_empty_for_a_missing_file_and_errors_for_an_unknown_id() {
        let fixture = SessionFixture::new("transcript_missing");
        let row = fixture.add_session("never-written.jsonl", false, "");

        let messages = session_transcript_for(&fixture.store, &row.id.to_string())
            .expect("a missing session file is not an error");
        assert!(messages.is_empty(), "got {messages:?}");

        let err = session_transcript_for(&fixture.store, "919191")
            .expect_err("an unindexed id has no transcript");
        assert!(err.contains("unknown session"), "got {err:?}");
    }

    // ── per-session actions (pause / delete) ────────────────────────────────

    // Deleting a session archives its transcript under the app's own folder: the
    // file belongs to pi, so a delete stays recoverable instead of shredding it.
    // Same-named transcripts never overwrite each other.
    #[test]
    fn archive_session_file_moves_the_transcript_into_the_trash() {
        let fixture = SessionFixture::new("archive_move");
        let data_dir = fixture.dir.clone();
        let transcript = fixture.path("s1.jsonl");
        std::fs::write(&transcript, "{\"type\":\"message\"}\n").expect("write transcript");

        let archived = archive_session_file(&data_dir, &transcript)
            .expect("archive")
            .expect("an existing file inside the app data dir is archived");

        assert!(
            !transcript.exists(),
            "the transcript leaves its indexed path"
        );
        assert_eq!(
            archived,
            data_dir.join("sessions").join(".trash").join("s1.jsonl"),
            "the archive lives in <app data dir>/sessions/.trash"
        );
        assert_eq!(
            std::fs::read_to_string(&archived).expect("read archived transcript"),
            "{\"type\":\"message\"}\n",
            "archiving preserves the content"
        );

        // A second transcript with the same file name gets its own unique name.
        let colliding = fixture.path("workspace").join("s1.jsonl");
        std::fs::write(&colliding, "second\n").expect("write second transcript");
        let second = archive_session_file(&data_dir, &colliding)
            .expect("archive")
            .expect("the colliding transcript is archived too");
        assert_ne!(second, archived, "a collision must never overwrite");
        assert_eq!(
            std::fs::read_to_string(&second).expect("read second archived transcript"),
            "second\n"
        );
    }

    // Negative guard: a delete never destroys a path the app did not create, and
    // a transcript pi never wrote is not an error.
    #[test]
    fn archive_session_file_leaves_foreign_and_missing_files_in_place() {
        let fixture = SessionFixture::new("archive_foreign");
        let outside = TempSeedDir::new("archive_outside");
        let foreign = outside.home().join("elsewhere.jsonl");
        std::fs::write(&foreign, "someone else's file\n").expect("write foreign file");

        assert_eq!(
            archive_session_file(&fixture.dir, &foreign).expect("archive"),
            None,
            "a file outside the app data dir is not ours to move"
        );
        assert!(foreign.exists(), "the foreign file stays where it was");

        let missing = fixture.path("never-written.jsonl");
        assert_eq!(
            archive_session_file(&fixture.dir, &missing).expect("archive"),
            None,
            "a missing transcript is nothing to archive"
        );
        assert!(
            !fixture.dir.join("sessions").exists(),
            "no archive folder is created for nothing"
        );
    }

    // The delete command: stop the process (if any), archive the transcript, then
    // drop the index row. An id that is not indexed is an error, never a silent
    // success.
    #[test]
    fn delete_session_core_archives_the_transcript_and_removes_the_row() {
        let fixture = SessionFixture::new("delete_session");
        let runtime = lifecycle_runtime();
        let row = fixture.add_session("s1.jsonl", true, "hola\n");

        let removed = delete_session_core(&fixture.store, &runtime, &fixture.dir, row.id)
            .expect("delete a session");
        assert!(removed, "the deleted session reports the removed row");
        assert_eq!(
            fixture.store.session_by_id(row.id).expect("lookup"),
            None,
            "the index row is gone"
        );
        assert!(
            fixture
                .store
                .list_sessions(fixture.workspace_id)
                .expect("list")
                .is_empty()
        );
        assert!(!fixture.path("s1.jsonl").exists(), "the file left its path");
        assert!(
            fixture
                .dir
                .join("sessions")
                .join(".trash")
                .join("s1.jsonl")
                .exists(),
            "the transcript is archived, not destroyed"
        );

        // A session pi never wrote still deletes: only the row existed.
        let bare = fixture.add_session("never-written.jsonl", false, "");
        assert!(
            delete_session_core(&fixture.store, &runtime, &fixture.dir, bare.id)
                .expect("delete a session without a transcript")
        );

        let err = delete_session_core(&fixture.store, &runtime, &fixture.dir, row.id)
            .expect_err("an unindexed id cannot be deleted");
        assert_eq!(err, format!("unknown session: {}", row.id));
    }

    // S3: a long session is capped at the newest 400 messages, oldest first, so
    // the bridge payload stays bounded and the UI still shows the recent tail.
    #[test]
    fn session_transcript_keeps_only_the_newest_four_hundred_messages() {
        let fixture = SessionFixture::new("transcript_cap");
        let mut raw = String::new();
        for index in 0..450 {
            raw.push_str(&format!(
                "{{\"type\":\"message\",\"message\":{{\"role\":\"user\",\"content\":\"m{index}\"}}}}\n"
            ));
        }
        let row = fixture.add_session("long.jsonl", true, &raw);

        let messages = session_transcript_for(&fixture.store, &row.id.to_string())
            .expect("transcript");
        assert_eq!(messages.len(), TRANSCRIPT_MESSAGE_LIMIT, "capped at 400");
        assert_eq!(
            messages.first().map(|message| message.text.as_str()),
            Some("m50"),
            "the oldest lines are dropped"
        );
        assert_eq!(
            messages.last().map(|message| message.text.as_str()),
            Some("m449"),
            "the newest message is kept"
        );
    }

    // S3 contract shape: `{ role, text, at }`, `at` is null when the file line
    // carried no stamp. The TS contract mirrors these keys exactly.
    #[test]
    fn transcript_message_serializes_as_role_text_and_at() {
        let json = serde_json::to_value(TranscriptMessage {
            role: "user".to_string(),
            text: "hola".to_string(),
            at: None,
        })
        .expect("serialize");
        assert_eq!(json["role"], serde_json::json!("user"));
        assert_eq!(json["text"], serde_json::json!("hola"));
        assert_eq!(json["at"], serde_json::Value::Null);
    }

    // ── session preview (instant first paint) ───────────────────────────────

    fn preview(role: &str, text: &str, reasoning: Option<&str>, at: Option<&str>) -> PreviewMessage {
        PreviewMessage {
            role: role.to_string(),
            text: text.to_string(),
            reasoning: reasoning.map(str::to_string),
            at: at.map(str::to_string),
        }
    }

    // The whole point of the preview: the newest turns come back exactly as they
    // went in, reasoning included — the part pi's JSONL file never stored.
    #[test]
    fn session_preview_round_trips_with_its_reasoning() {
        let fixture = SessionFixture::new("preview_round_trip");
        let row = fixture.add_session("preview.jsonl", true, "");
        let messages = vec![
            preview("user", "¿cómo va el chat?", None, Some("2026-05-01T10:00:00Z")),
            preview(
                "assistant",
                "Pinta desde el preview.",
                Some("El usuario quiere que abra rápido."),
                Some("2026-05-01T10:00:02Z"),
            ),
            preview("tool", "read src/App.vue", None, None),
        ];

        assert!(
            save_session_preview_for(&fixture.store, row.id, &messages),
            "an indexed session stores its preview"
        );
        assert_eq!(
            load_session_preview_for(&fixture.store, row.id),
            Some(messages),
            "the preview reads back message by message, reasoning included"
        );

        // A later snapshot of the same session replaces the previous one.
        let shorter = vec![preview("user", "solo esto", None, None)];
        assert!(save_session_preview_for(&fixture.store, row.id, &shorter));
        assert_eq!(load_session_preview_for(&fixture.store, row.id), Some(shorter));
    }

    // A snapshot written before pi's file grew is no longer worth painting: the
    // file has turns the preview does not, so the caller reads the file instead.
    #[test]
    fn session_preview_is_dropped_once_pis_file_outgrows_it() {
        let fixture = SessionFixture::new("preview_stale");
        let row = fixture.add_session("stale.jsonl", true, "");
        // Written by hand with a stamp the file's own mtime is well past.
        assert!(
            fixture
                .store
                .upsert_session_preview(row.id, "[]", "2020-01-01T00:00:00Z")
                .expect("store a stale preview")
        );
        assert_eq!(
            load_session_preview_for(&fixture.store, row.id),
            None,
            "a preview pi's file outgrew is not painted"
        );
    }

    // A corrupt snapshot is not an error: the caller falls back to pi's file, so a
    // preview written by an older build can never keep a session from opening.
    #[test]
    fn session_preview_answers_none_when_the_stored_json_is_corrupt() {
        let fixture = SessionFixture::new("preview_corrupt");
        let row = fixture.add_session("corrupt.jsonl", true, "");
        assert!(
            fixture
                .store
                .upsert_session_preview(row.id, "{not json at all", &now_iso())
                .expect("store the corrupt preview")
        );
        assert_eq!(load_session_preview_for(&fixture.store, row.id), None);
    }

    // Saving is best-effort: an id that is not indexed stores nothing and says so
    // instead of failing the turn that produced the snapshot.
    #[test]
    fn saving_a_preview_for_an_unindexed_session_writes_nothing() {
        let fixture = SessionFixture::new("preview_unindexed");
        assert!(!save_session_preview_for(
            &fixture.store,
            9_999,
            &[preview("user", "hola", None, None)]
        ));
        assert_eq!(load_session_preview_for(&fixture.store, 9_999), None);
    }

    // The preview is bounded: a long conversation keeps its newest messages, which
    // are the ones an open is looking at, and unknown roles are dropped instead of
    // costing a paint the UI cannot render.
    #[test]
    fn session_preview_keeps_the_newest_messages_and_only_renderable_roles() {
        let fixture = SessionFixture::new("preview_cap");
        let row = fixture.add_session("cap.jsonl", true, "");
        let mut messages: Vec<PreviewMessage> = (0..PREVIEW_MESSAGE_LIMIT + 5)
            .map(|index| preview("user", &format!("m{index}"), None, None))
            .collect();
        messages.push(preview("system", "no se renderiza", None, None));

        assert!(save_session_preview_for(&fixture.store, row.id, &messages));
        let stored = load_session_preview_for(&fixture.store, row.id).expect("preview");
        assert_eq!(stored.len(), PREVIEW_MESSAGE_LIMIT, "capped");
        assert_eq!(stored.first().map(|m| m.text.as_str()), Some("m5"));
        assert_eq!(
            stored.last().map(|m| m.text.as_str()),
            Some(format!("m{}", PREVIEW_MESSAGE_LIMIT + 4).as_str()),
            "a role the transcript does not render is not stored"
        );
    }

    // Contract shape: `{ role, text, reasoning, at }`, `null` for what was never
    // there. The TS mirror (`PreviewMessage` in `src/contracts/types.ts`) is these
    // exact keys.
    #[test]
    fn preview_message_serializes_as_role_text_reasoning_and_at() {
        let json = serde_json::to_value(preview("assistant", "hola", None, None))
            .expect("serialize");
        assert_eq!(json["role"], serde_json::json!("assistant"));
        assert_eq!(json["text"], serde_json::json!("hola"));
        assert_eq!(json["reasoning"], serde_json::Value::Null);
        assert_eq!(json["at"], serde_json::Value::Null);
        // And the null fields parse back as absent options, so a payload with no
        // reasoning is not a decode error.
        let parsed: PreviewMessage = serde_json::from_value(json).expect("deserialize");
        assert_eq!(parsed.reasoning, None);
    }

    // ── MCP configuration (F0 of `tool-drawer-and-mcp.md`) ─────────────────

    /// One finished `pi mcp list --json` call, as the bridge would report it.
    fn cli_result(stdout: &str, exit_code: Option<i32>, timed_out: bool) -> PiCliResult {
        PiCliResult {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code,
            timed_out,
            truncated: false,
        }
    }

    const MCP_LIST_PAYLOAD: &str = r#"{
      "servers": [
        { "name": "engram", "scope": "global", "enabled": true,
          "transport": "/opt/homebrew/bin/engram mcp --tools=agent",
          "state": "connected", "tools": ["mem_save"] }
      ],
      "errors": []
    }"#;

    // S1: `pi mcp list` exits 1 whenever a server is unhealthy, and that answer is
    // exactly what the pane must paint: the exit code never discards the JSON.
    #[test]
    fn mcp_list_from_reads_the_json_even_when_the_call_exits_non_zero() {
        for exit_code in [Some(0), Some(1), None] {
            let servers = mcp_list_from(&cli_result(MCP_LIST_PAYLOAD, exit_code, false))
                .expect("the JSON is the answer, whatever the exit code");
            assert_eq!(servers.len(), 1, "exit: {exit_code:?}");
            assert_eq!(servers[0].name, "engram");
            assert_eq!(servers[0].state, mcp::McpState::Ready);
        }
    }

    // S1: a listing that timed out has nothing to parse, so it is reported as an
    // error instead of an empty list that would read as "no MCP servers".
    #[test]
    fn mcp_list_from_reports_a_timeout_as_an_error() {
        let err = mcp_list_from(&cli_result("", None, true)).expect_err("a timeout is an error");
        assert!(err.contains("did not finish"), "got: {err}");
    }

    // S1: garbage in the payload is "no servers", never a panic and never a
    // failed call.
    #[test]
    fn mcp_list_from_answers_an_empty_list_for_garbage() {
        assert!(
            mcp_list_from(&cli_result("not json", Some(1), false))
                .expect("garbage is not a failure")
                .is_empty()
        );
    }

    // S1: a mutating call reports pi's own diagnostic, not a bare failure.
    #[test]
    fn check_mcp_mutation_reports_the_exit_code_and_stderr() {
        let ok = cli_result("added\n", Some(0), false);
        assert_eq!(check_mcp_mutation(&ok, "add"), Ok(()));

        let mut failed = cli_result("", Some(1), false);
        failed.stderr = "No global MCP server named \"x\".\n".to_string();
        let err = check_mcp_mutation(&failed, "remove").expect_err("a non-zero exit fails");
        assert!(err.contains("exit 1"), "got: {err}");
        assert!(err.contains("No global MCP server named"), "got: {err}");

        // A child terminated by a signal has no code: still a failure, and never
        // an empty diagnostic.
        let killed = cli_result("", None, false);
        let err = check_mcp_mutation(&killed, "logout").expect_err("no code is not success");
        assert!(err.contains("no diagnostic"), "got: {err}");
    }

    // S4 of `webview-control.md`: an entry that is not there is exactly the state
    // the app wants, so a `remove` that finds nothing must not be reported as a
    // failure — it is the normal answer of a run that never registered.
    #[test]
    fn nothing_to_remove_reads_pi_refusing_an_absent_entry() {
        let mut absent = cli_result("", Some(1), false);
        absent.stderr =
            "No global MCP server named \"nabla-browser\" in /home/somebody/.pi/agent/mcp.json.\n"
                .to_string();
        assert!(nothing_to_remove(&absent));

        // A real failure — pi could not rewrite the file — is not that answer.
        let mut unreadable = cli_result("", Some(1), false);
        unreadable.stderr =
            "Could not update /home/somebody/.pi/agent/mcp.json: EACCES\n".to_string();
        assert!(!nothing_to_remove(&unreadable));

        // A successful remove is not "nothing to remove" either.
        assert!(!nothing_to_remove(&cli_result(
            "Removed global MCP server \"nabla-browser\"\n",
            Some(0),
            false
        )));
    }

    // S1: the file path is absolute and names pi's own file for each scope.
    #[test]
    fn mcp_file_path_names_the_global_and_project_files() {
        let home = Path::new("/home/somebody");
        let project = Path::new("/work/project");

        let global = mcp_file_path_for(home, project, McpScope::Global).expect("absolute home");
        assert!(Path::new(&global).is_absolute(), "got: {global}");
        assert!(global.ends_with(".pi/agent/mcp.json"), "got: {global}");

        let local = mcp_file_path_for(home, project, McpScope::Project).expect("absolute project");
        assert_eq!(local, "/work/project/.pi/mcp.json");

        // A relative root would display a path the app cannot open.
        assert!(mcp_file_path_for(Path::new("home"), project, McpScope::Global).is_err());
    }

    // ── composer inputs: effort, / commands and @ mentions ────────────────
    // (S2, S3 of `composer-inputs.md`)

    // S2: pi answers `{"levels": [...]}` for the ACTIVE model
    // (pi docs/rpc-commands.md, get_available_thinking_levels). Those words are
    // what the effort selector offers, in pi's order: Nabla proposes, pi decides.
    #[test]
    fn thinking_levels_from_response_reads_the_model_levels_in_order() {
        let data = serde_json::json!({ "levels": ["off", "minimal", "high"] });
        assert_eq!(
            thinking_levels_from_response(Some(&data)),
            vec!["off".to_string(), "minimal".to_string(), "high".to_string()]
        );

        // Nothing to offer is an empty list, never an invented one: a model
        // without reasoning support answers `["off"]`, and an answer that is
        // not a list of strings leaves the selector with nothing to choose.
        assert!(thinking_levels_from_response(None).is_empty());
        assert!(
            thinking_levels_from_response(Some(&serde_json::json!({ "levels": "high" })))
                .is_empty()
        );
        let mixed = serde_json::json!({ "levels": ["low", 7, "", " max "] });
        assert_eq!(
            thinking_levels_from_response(Some(&mixed)),
            vec!["low".to_string(), "max".to_string()],
            "only real, non-empty words are levels"
        );
    }

    // S3: pi answers `{commands: [{name, description?, source, sourceInfo}]}`.
    // Nabla keeps the three fields the list renders and drops the rest — it does
    // not interpret a command, it names it (the `prompt` command runs it).
    #[test]
    fn session_commands_from_response_keeps_name_description_and_source() {
        let data = serde_json::json!({
            "commands": [
                {
                    "name": "fix-tests",
                    "description": "Fix failing tests",
                    "source": "prompt",
                    "sourceInfo": { "path": "/work/.pi/prompts/fix-tests.md" }
                },
                { "name": "skill:commit", "source": "skill" },
                { "name": "review", "source": "extension" },
                // No name is not a command: nothing could be completed or run.
                { "description": "nameless", "source": "prompt" },
                { "name": "   ", "source": "prompt" }
            ]
        });
        let commands = session_commands_from_response(Some(&data));
        assert_eq!(commands.len(), 3, "a command needs a name: {commands:?}");
        assert_eq!(commands[0].name, "fix-tests");
        assert_eq!(commands[0].description.as_deref(), Some("Fix failing tests"));
        assert_eq!(commands[0].source, "prompt");
        // An absent description stays absent instead of becoming an empty string
        // the list would render as a blank line.
        assert_eq!(commands[1].description, None);
        assert_eq!(commands[1].source, "skill");
        assert_eq!(commands[2].source, "extension");

        // The webview mirrors this shape field by field (R4): no `sourceInfo`,
        // no `null` where pi sent nothing.
        let wire = serde_json::to_value(&commands[1]).expect("serialize");
        assert_eq!(wire, serde_json::json!({ "name": "skill:commit", "source": "skill" }));

        // An answer that carries no list at all is empty, never an error: a
        // session that has nothing to offer shows no list.
        assert!(session_commands_from_response(None).is_empty());
        assert!(
            session_commands_from_response(Some(&serde_json::json!({ "commands": 3 }))).is_empty()
        );
    }

    // S2: a word that is not a level pi documents is refused before anything is
    // written — picking one on the human's behalf would change the agent's
    // reasoning on a typo.
    #[test]
    fn set_thinking_level_rejects_an_unknown_word_and_writes_nothing() {
        let fixture = SessionFixture::new("thinking_level_bad");
        let runtime = lifecycle_runtime();

        for bad in ["", "  ", "maximo", "on", "none"] {
            let err = set_thinking_level_for(&fixture.config_dir(), &runtime, "1", bad)
                .expect_err("reject");
            assert!(
                err.contains("unknown thinking level"),
                "{bad:?} must be rejected with a reason: {err}"
            );
        }
        assert!(
            !settings_file(&fixture.config_dir()).exists(),
            "a rejected level writes no settings file"
        );
    }

    // S2: with no live child there is nothing to change in a session, but the
    // choice is still the default the next spawn uses — that is the whole point
    // of persisting it. The answer is what the file now holds.
    #[test]
    fn set_thinking_level_without_a_live_child_persists_the_default() {
        let fixture = SessionFixture::new("thinking_level_file");
        let runtime = lifecycle_runtime();

        assert_eq!(
            set_thinking_level_for(&fixture.config_dir(), &runtime, "1", "high").expect("set"),
            "high"
        );
        let stored = settings::load(&settings_file(&fixture.config_dir())).expect("load");
        assert_eq!(stored.thinking_level, "high");
        assert_eq!(stored.thinking(), Some("high"));

        // The word the human picked is normalized, not stored twice in two cases.
        assert_eq!(
            set_thinking_level_for(&fixture.config_dir(), &runtime, "1", "  OFF ").expect("set"),
            "off"
        );
        assert_eq!(
            settings::load(&settings_file(&fixture.config_dir()))
                .expect("load")
                .thinking_level,
            "off"
        );
    }

    // S2: a live session changes WITHOUT a respawn: the level leaves as the
    // protocol's own `set_thinking_level` request on the child's stdin, and the
    // file keeps it as the default for the next session. Asserted on the bytes
    // the child received, not on the builder.
    #[cfg(unix)]
    #[test]
    fn set_thinking_level_reaches_a_live_child_and_persists() {
        let fixture = SessionFixture::new("thinking_level_live");
        let script = fixture.write_fake_pi("fake-pi.sh");
        fixture.write_settings(&script);
        let row = fixture.add_session("s1.jsonl", true, "");
        let runtime = lifecycle_runtime();
        let key = row.id.to_string();
        let gate = registered_gate();

        block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &gate,
            &key,
        ))
        .expect("open session");
        assert_eq!(
            process_attempts(&fixture),
            1,
            "S2: a level change must never respawn the session"
        );

        assert_eq!(
            set_thinking_level_for(&fixture.config_dir(), &runtime, &key, "high").expect("set"),
            "high"
        );

        let sent = recorded_stdin(&fixture, "fake-pi.sh");
        let request = sent
            .iter()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("json line"))
            .find(|value| value["type"] == "set_thinking_level")
            .expect("the child must receive set_thinking_level");
        assert_eq!(request["level"], "high", "got: {request}");
        assert_eq!(
            request["id"].as_str().map(|id| id.is_empty()),
            Some(false),
            "the request is correlated by an id Nabla allocated: {request}"
        );
        assert_eq!(
            settings::load(&settings_file(&fixture.config_dir()))
                .expect("load")
                .thinking_level,
            "high",
            "and the file keeps it as the default for the next session"
        );
        assert_eq!(process_attempts(&fixture), 1, "still no respawn");

        runtime.stop_all();
    }

    // S2: the levels the selector offers are the ones the session's model
    // accepts, read through the child that owns the model.
    #[cfg(unix)]
    #[test]
    fn session_thinking_levels_proxies_the_live_session() {
        let fixture = SessionFixture::new("thinking_levels_proxy");
        let script = fixture.write_fake_pi("fake-pi.sh");
        fixture.write_settings(&script);
        let row = fixture.add_session("s1.jsonl", true, "");
        let runtime = lifecycle_runtime();
        let key = row.id.to_string();

        // A session with no process cannot answer, and that is an error: an empty
        // list would claim the model supports no level at all.
        assert!(
            session_thinking_levels_for(&runtime, &key).is_err(),
            "a stopped session must not answer with an empty list"
        );

        block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &registered_gate(),
            &key,
        ))
        .expect("open session");

        assert_eq!(
            session_thinking_levels_for(&runtime, &key).expect("levels"),
            vec![
                "off".to_string(),
                "minimal".to_string(),
                "low".to_string(),
                "medium".to_string(),
                "high".to_string()
            ]
        );

        runtime.stop_all();
    }

    // S3: the session's commands cross the seam with their source, so the list
    // can wear a badge without Nabla ever reading a command's body.
    #[cfg(unix)]
    #[test]
    fn session_commands_proxies_the_live_session() {
        let fixture = SessionFixture::new("session_commands_proxy");
        let script = fixture.write_fake_pi("fake-pi.sh");
        fixture.write_settings(&script);
        let row = fixture.add_session("s1.jsonl", true, "");
        let runtime = lifecycle_runtime();
        let key = row.id.to_string();

        assert!(
            session_commands_for(&runtime, &key).is_err(),
            "a stopped session answers no commands, and that is an error"
        );

        block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &registered_gate(),
            &key,
        ))
        .expect("open session");

        let commands = session_commands_for(&runtime, &key).expect("commands");
        let pairs: Vec<(&str, &str)> = commands
            .iter()
            .map(|command| (command.name.as_str(), command.source.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![("fix-tests", "prompt"), ("skill:commit", "skill")],
            "pi's list crosses with its own sources: {commands:?}"
        );
        assert_eq!(
            commands[0].description.as_deref(),
            Some("Fix failing tests")
        );

        runtime.stop_all();
    }

    // ── S2 of `models-by-provider.md`: switching the LIVE session's model ───

    // S2: the pick reaches the live child as the protocol's own `set_model`
    // request — the same correlated line every other request uses — and what
    // comes back is the model pi says it is running, not an echo of the wish.
    // Asserted on the bytes the child received, and on the attempt count: a model
    // switch is not a respawn.
    #[cfg(unix)]
    #[test]
    fn set_session_model_reaches_a_live_child_and_returns_pis_answer() {
        let fixture = SessionFixture::new("set_model_live");
        let script = fixture.write_fake_pi("fake-pi.sh");
        fixture.write_settings(&script);
        let row = fixture.add_session("s1.jsonl", true, "");
        let runtime = lifecycle_runtime();
        let key = row.id.to_string();

        block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &registered_gate(),
            &key,
        ))
        .expect("open session");
        assert_eq!(process_attempts(&fixture), 1);

        let info = set_session_model_for(&runtime, &key, "anthropic", "claude-sonnet-4-5")
            .expect("set model");

        let sent = recorded_stdin(&fixture, "fake-pi.sh");
        let request = sent
            .iter()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("json line"))
            .find(|value| value["type"] == "set_model")
            .expect("the child must receive set_model");
        assert_eq!(request["provider"], "anthropic", "got: {request}");
        assert_eq!(request["modelId"], "claude-sonnet-4-5", "got: {request}");
        assert_eq!(
            request["id"].as_str().map(|id| id.is_empty()),
            Some(false),
            "the request is correlated by an id Nabla allocated: {request}"
        );

        assert_eq!(
            info,
            SessionModelInfo {
                provider: "anthropic".to_string(),
                id: "claude-sonnet-4-5".to_string(),
                name: "Claude Sonnet 4.5".to_string(),
            },
            "the answer is pi's model, read from its response"
        );
        assert_eq!(
            process_attempts(&fixture),
            1,
            "S2: a model switch must never respawn the session"
        );

        runtime.stop_all();
    }

    // S2: with no live child there is nothing to switch, and the command says so
    // instead of reporting a success that never happened — the persisted default
    // for the next spawn is the composer's own write, not this one.
    #[test]
    fn set_session_model_refuses_a_session_that_is_not_live() {
        let fixture = SessionFixture::new("set_model_dead");
        let runtime = lifecycle_runtime();
        let key = "7";

        let err = set_session_model_for(&runtime, key, "anthropic", "claude-sonnet-4-5")
            .expect_err("refuse");
        assert!(err.contains("not live"), "a dead session is refused: {err}");
        assert!(err.contains(key), "the error names the session: {err}");
        assert!(
            !settings_file(&fixture.config_dir()).exists(),
            "the command writes no settings: the composer owns that default"
        );
    }

    // S2: pi's answer is the source of truth about the model it now runs. A
    // response that names none (an older or newer pi) still reports the pair the
    // switch was asked for, and a blank name falls back to the id: the switch did
    // happen either way, and a label is never empty.
    #[test]
    fn set_session_model_reads_the_model_pi_names() {
        let named = serde_json::json!({
            "provider": "anthropic",
            "id": "claude-opus-4-1",
            "name": "Claude Opus 4.1",
            "api": "anthropic-messages",
            "contextWindow": 200000,
        });
        assert_eq!(
            model_info_from_data(Some(&named), "anthropic", "alias"),
            SessionModelInfo {
                provider: "anthropic".to_string(),
                id: "claude-opus-4-1".to_string(),
                name: "Claude Opus 4.1".to_string(),
            }
        );

        let nameless = serde_json::json!({ "provider": "custom", "id": "m-1", "name": "  " });
        assert_eq!(
            model_info_from_data(Some(&nameless), "custom", "m"),
            SessionModelInfo {
                provider: "custom".to_string(),
                id: "m-1".to_string(),
                name: "m-1".to_string(),
            }
        );

        assert_eq!(
            model_info_from_data(None, "custom", "m"),
            SessionModelInfo {
                provider: "custom".to_string(),
                id: "m".to_string(),
                name: "m".to_string(),
            }
        );
    }

    // Every session command is scoped to the workspace that indexes it: a row of
    // another workspace must never reach this session's runtime key.
    #[test]
    fn set_session_model_refuses_a_session_of_another_workspace() {
        let fixture = SessionFixture::new("set_model_guard");
        let row = fixture.add_session("s1.jsonl", true, "");
        let other = fixture
            .store
            .add_workspace("/tmp/another-workspace", None)
            .expect("add workspace");

        let err = ensure_session_in_workspace(&fixture.store, other.id, row.id)
            .expect_err("refuse");
        assert!(
            err.contains("is not indexed in workspace"),
            "got: {err}"
        );

        // The workspace that does index it is accepted: the guard filters, it is
        // not a wall.
        ensure_session_in_workspace(&fixture.store, fixture.workspace_id, row.id)
            .expect("own workspace");
    }

    // ── F2 of `tool-drawer-and-mcp.md`: the workspace terminal ─────────────

    // S2: a terminal starts in the workspace it was opened for. A workspace the
    // app cannot read is refused before any pty exists, so no shell is ever
    // opened in a directory nobody asked for.
    #[test]
    fn terminal_cwd_is_the_workspace_and_must_exist() {
        let fixture = TempSeedDir::new("terminal_cwd");
        let workspace = fixture.home();

        let cwd = terminal_cwd(&workspace.to_string_lossy()).expect("an existing workspace");
        assert_eq!(cwd, workspace.canonicalize().expect("canonical home"));
        assert!(cwd.is_dir(), "a terminal needs a directory to start in");

        let gone = workspace.join("gone");
        let err = terminal_cwd(&gone.to_string_lossy())
            .expect_err("a workspace that no longer exists is refused");
        assert!(err.contains("not readable"), "got: {err}");

        // A relative path would silently open the shell in the app's own cwd.
        let err = terminal_cwd("relative/workspace").expect_err("a relative path is refused");
        assert!(err.contains("absolute"), "got: {err}");
    }

    // ── the on-demand command probe (a workspace, no session) ──────────────

    // The reserved prefix is what makes a probe key impossible to confuse with a
    // session key, and a session key impossible to mistake for a probe: the two
    // spaces cannot meet, because one is always a decimal row id.
    #[test]
    fn a_probe_key_can_never_be_a_session_id() {
        for workspace_id in [0, 1, 42, i64::MAX] {
            let key = probe_key(workspace_id);
            assert!(is_probe_key(&key), "the probe key carries the prefix: {key}");
            assert!(
                key.parse::<i64>().is_err(),
                "a session key is a decimal row id, so this can never be one: {key}"
            );
            // ... and no session id reads as a probe.
            assert!(!is_probe_key(&workspace_id.to_string()));
        }
        assert_eq!(probe_key(7), "probe:7");
        assert_eq!(probe_file_name(7), ".probe-7.jsonl");
    }

    // The whole lifecycle: spawn once, ask `get_commands`, parse the answer, stop
    // the child and delete the scratch transcript — and never a session row.
    #[cfg(unix)]
    #[test]
    fn a_probe_answers_a_workspaces_commands_and_leaves_nothing_behind() {
        let fixture = SessionFixture::new("probe_answers");
        let script = fixture.write_probe_pi("probe-pi.sh", 0);
        fixture.write_settings(&script);
        let runtime = lifecycle_runtime();
        let probes = Arc::new(CommandProbes::new());
        // A transcript a previous run could have left behind: the probe owns the
        // path, so it must not survive this one either.
        std::fs::create_dir_all(fixture.probe_scratch().parent().expect("sessions dir"))
            .expect("create sessions dir");
        std::fs::write(fixture.probe_scratch(), "leftover").expect("write leftover");

        let commands = workspace_commands_core(
            &probes,
            &fixture.store,
            &fixture.config_dir(),
            &fixture.dir,
            &runtime,
            fixture.workspace_id,
        )
        .expect("the probe answers");

        let rows: Vec<(&str, Option<&str>, &str)> = commands
            .iter()
            .map(|command| {
                (
                    command.name.as_str(),
                    command.description.as_deref(),
                    command.source.as_str(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("fix-tests", Some("Fix failing tests"), "prompt"),
                // An absent description stays absent instead of becoming a blank
                // subtitle, and the source crosses verbatim.
                ("skill:commit", None, "skill")
            ],
            "the answer is pi's own list"
        );

        // One child was spawned, and it was asked with the reserved key.
        assert_eq!(process_attempts(&fixture), 1, "one probe, one child");
        let stdin = recorded_stdin(&fixture, "probe-pi.sh");
        assert!(
            stdin
                .iter()
                .any(|line| line.contains("\"type\":\"get_commands\"")),
            "the child is asked for its commands: {stdin:?}"
        );

        // A probe is not a session: no row, so no ghost in the sidebar.
        assert!(
            fixture
                .store
                .list_sessions(fixture.workspace_id)
                .expect("list sessions")
                .is_empty(),
            "the probe must never index a session"
        );
        // Neither the scratch transcript nor the child survives the question.
        assert!(
            !fixture.probe_scratch().exists(),
            "the scratch transcript is deleted: {}",
            fixture.probe_scratch().display()
        );
        assert!(
            runtime.live_ids().is_empty(),
            "the probe child does not survive: {:?}",
            runtime.live_ids()
        );
        runtime.stop_all();
    }

    // One probe per workspace at a time: the second caller joins the first one's
    // child instead of starting its own, and adopts the same answer.
    #[cfg(unix)]
    #[test]
    fn a_concurrent_probe_spawns_no_second_child() {
        let fixture = SessionFixture::new("probe_concurrent");
        // Slow enough that both callers are inside the probe at once.
        let script = fixture.write_probe_pi("probe-pi.sh", 1);
        fixture.write_settings(&script);
        let runtime = lifecycle_runtime();
        let probes = Arc::new(CommandProbes::new());
        let barrier = Arc::new(std::sync::Barrier::new(2));

        let answers = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..2 {
                let probes = Arc::clone(&probes);
                let store = fixture.store.clone();
                let config_dir = fixture.config_dir();
                let data_dir = fixture.dir.clone();
                let runtime = runtime.clone();
                let barrier = Arc::clone(&barrier);
                let workspace_id = fixture.workspace_id;
                handles.push(scope.spawn(move || {
                    // Both threads reach the guard together, from the same point.
                    barrier.wait();
                    workspace_commands_core(
                        &probes,
                        &store,
                        &config_dir,
                        &data_dir,
                        &runtime,
                        workspace_id,
                    )
                }));
            }
            handles
                .into_iter()
                .map(|handle| handle.join().expect("the probe thread panicked"))
                .collect::<Vec<_>>()
        });

        for answer in &answers {
            let commands = answer.as_ref().expect("a probe never errors here");
            assert_eq!(
                commands.len(),
                2,
                "both callers get the shared answer, got {commands:?}"
            );
        }
        assert_eq!(&answers[0], &answers[1], "one outcome, shared");
        assert_eq!(
            process_attempts(&fixture),
            1,
            "a second concurrent call spawns no second child"
        );
        assert!(
            fixture
                .store
                .list_sessions(fixture.workspace_id)
                .expect("list sessions")
                .is_empty(),
            "no session row, ever"
        );
        assert!(!fixture.probe_scratch().exists(), "one cleanup for one probe");
        assert!(runtime.live_ids().is_empty(), "no child survives");
        runtime.stop_all();
    }

    // The composer asks this while someone is typing, so a probe that cannot be
    // made is an empty list: never an error in front of a keystroke.
    #[cfg(unix)]
    #[test]
    fn a_probe_without_a_pi_binary_answers_an_empty_list() {
        let fixture = SessionFixture::new("probe_no_binary");
        fixture.write_settings(Path::new("/nonexistent/nabla-test-pi"));
        let runtime = lifecycle_runtime();
        let probes = Arc::new(CommandProbes::new());

        let commands = workspace_commands_core(
            &probes,
            &fixture.store,
            &fixture.config_dir(),
            &fixture.dir,
            &runtime,
            fixture.workspace_id,
        )
        .expect("a missing binary is not an error");

        assert!(commands.is_empty(), "got {commands:?}");
        assert!(
            fixture
                .store
                .list_sessions(fixture.workspace_id)
                .expect("list sessions")
                .is_empty(),
            "a failed probe still writes no session row"
        );
        assert!(!fixture.probe_scratch().exists(), "and leaves no scratch file");
        assert!(runtime.live_ids().is_empty());
    }

    // Same stance for a child that starts and dies: an empty list, not a failure.
    #[cfg(unix)]
    #[test]
    fn a_probe_child_that_dies_answers_an_empty_list() {
        let fixture = SessionFixture::new("probe_dead_child");
        let script = fixture.write_dead_pi("dead-pi.sh");
        fixture.write_settings(&script);
        let runtime = lifecycle_runtime();
        let probes = Arc::new(CommandProbes::new());

        let commands = workspace_commands_core(
            &probes,
            &fixture.store,
            &fixture.config_dir(),
            &fixture.dir,
            &runtime,
            fixture.workspace_id,
        )
        .expect("a child that dies is not an error");

        assert!(commands.is_empty(), "got {commands:?}");
        assert!(!fixture.probe_scratch().exists());
        assert!(runtime.live_ids().is_empty());
        assert!(
            fixture
                .store
                .list_sessions(fixture.workspace_id)
                .expect("list sessions")
                .is_empty()
        );
    }

    // An id the store does not index is a contract violation, not a probe
    // failure: it is reported, and it costs no child at all.
    #[cfg(unix)]
    #[test]
    fn a_probe_for_an_unindexed_workspace_errors_without_spawning() {
        let fixture = SessionFixture::new("probe_unknown");
        let script = fixture.write_probe_pi("probe-pi.sh", 0);
        fixture.write_settings(&script);
        let runtime = lifecycle_runtime();
        let probes = Arc::new(CommandProbes::new());

        let err = workspace_commands_core(
            &probes,
            &fixture.store,
            &fixture.config_dir(),
            &fixture.dir,
            &runtime,
            424242,
        )
        .expect_err("an unindexed workspace cannot be probed");

        assert!(err.contains("unknown workspace"), "got {err:?}");
        assert_eq!(process_attempts(&fixture), 0, "nothing was spawned");
        assert!(runtime.live_ids().is_empty());
    }

    // ── subagent sessions (H3 of `subagent-helpers.md`) ─────────────────────

    /// A store with one workspace and one indexed session, plus the data dir the
    /// extension would publish its manifests into: everything the linkage needs and
    /// nothing else.
    struct SubagentFixture {
        dir: TempSeedDir,
        store: Store,
        workspace_id: i64,
        parent: SessionRow,
    }

    impl SubagentFixture {
        fn new(tag: &str) -> Self {
            let dir = TempSeedDir::new(tag);
            let store = dir.store();
            let workspace = store.add_workspace("/tmp/proj-subagents", None).expect("ws");
            let parent = store
                .upsert_session(&SessionNew {
                    workspace_id: workspace.id,
                    pi_session_path: "/sessions/parent-chat.jsonl".into(),
                    title: Some("the chat".into()),
                    model: Some("gpt-6.1-sol".into()),
                    started_at: "2026-07-01T09:00:00Z".into(),
                    updated_at: "2026-07-01T09:30:00Z".into(),
                })
                .expect("parent session");
            SubagentFixture {
                dir,
                store,
                workspace_id: workspace.id,
                parent,
            }
        }

        fn data_dir(&self) -> std::path::PathBuf {
            self.dir.0.join("data")
        }

        /// The manifest the extension writes, under a name that does NOT encode
        /// the parent: the link is the `parentSessionPath` field, never the file
        /// name, so a rename cannot silently unparent a child.
        fn write_manifest(&self, file: &str, body: &str) {
            let dir = self.data_dir().join("subagents");
            std::fs::create_dir_all(&dir).expect("manifest dir");
            std::fs::write(dir.join(file), body).expect("manifest");
        }
    }

    // H3.1/H3.2: the manifest is the whole host contract, so parsing it is the one
    // place a newer or older extension could break the sidebar. A line that names
    // no session file, a body that is not an array and a field this build does not
    // know must all be readable without failing anything.
    #[test]
    fn manifest_entries_reads_the_host_contract() {
        let entries = manifest_entries(
            r#"[
              {
                "name": "review",
                "agent": "reviewer",
                "sessionPath": "/data/sessions/subagents/a.jsonl",
                "status": "running",
                "startedAt": "2026-07-01T10:00:00.000Z",
                "endedAt": null,
                "parentSessionPath": "/sessions/parent-chat.jsonl",
                "somethingNewer": 42
              },
              { "agent": "reviewer" }
            ]"#,
        );
        assert_eq!(entries.len(), 2, "both lines parse: {entries:?}");
        assert_eq!(entries[0].name, "review");
        assert_eq!(entries[0].agent, "reviewer");
        assert_eq!(
            entries[0].session_path,
            "/data/sessions/subagents/a.jsonl"
        );
        assert_eq!(entries[0].status, "running");
        assert_eq!(entries[0].ended_at, None);
        assert_eq!(
            entries[0].parent_session_path.as_deref(),
            Some("/sessions/parent-chat.jsonl")
        );
        // A half-written line keeps the fields it has: the sync skips it for
        // having no path, it does not fail the listing.
        assert_eq!(entries[1].session_path, "");

        assert!(manifest_entries("").is_empty());
        assert!(manifest_entries("{}").is_empty(), "an object is not a list");
        assert!(manifest_entries("not json").is_empty());
    }

    // H3.2/H3.6: a child the manifest attributes to a chat is indexed under that
    // chat, in its workspace, and the answer carries the run's identity.
    #[test]
    fn subagent_sessions_links_a_child_to_the_manifest_parent() {
        let fixture = SubagentFixture::new("subagent_link");
        fixture.write_manifest(
            "parent-chat.json",
            r#"[
              {
                "name": "review",
                "agent": "reviewer",
                "sessionPath": "/data/sessions/subagents/child-a.jsonl",
                "status": "running",
                "startedAt": "2026-07-01T10:00:00Z",
                "endedAt": null,
                "parentSessionPath": "/sessions/parent-chat.jsonl"
              }
            ]"#,
        );

        let children = subagent_sessions_for(
            &fixture.store,
            &fixture.data_dir(),
            fixture.workspace_id,
            fixture.parent.id,
        )
        .expect("list children");

        assert_eq!(children.len(), 1, "got {children:?}");
        assert_eq!(children[0].name, "review");
        assert_eq!(children[0].agent, "reviewer");
        assert_eq!(
            children[0].session_path,
            "/data/sessions/subagents/child-a.jsonl"
        );
        assert_eq!(children[0].status, "running");
        // The row really is a child row of the index, not just a payload.
        let row = fixture
            .store
            .session_by_id(children[0].session_id)
            .expect("by id")
            .expect("the row");
        assert_eq!(row.parent_session_id, Some(fixture.parent.id));
        assert_eq!(row.workspace_id, fixture.workspace_id);
        assert_eq!(row.title.as_deref(), Some("reviewer"));
    }

    // H3.3: the status is live — read from the manifest on every call — while the
    // row behind it is not rewritten, so refreshing a sidebar costs no writes.
    #[test]
    fn subagent_sessions_reports_live_status_without_adding_a_row_twice() {
        let fixture = SubagentFixture::new("subagent_status");
        let running = r#"[
              {
                "name": "review",
                "agent": "reviewer",
                "sessionPath": "/data/sessions/subagents/child-a.jsonl",
                "status": "running",
                "startedAt": "2026-07-01T10:00:00Z",
                "endedAt": null,
                "parentSessionPath": "/sessions/parent-chat.jsonl"
              },
              {
                "name": "tests",
                "agent": "tester",
                "sessionPath": "/data/sessions/subagents/child-b.jsonl",
                "status": "done",
                "startedAt": "2026-07-01T10:01:00Z",
                "endedAt": "2026-07-01T10:04:00Z",
                "parentSessionPath": "/sessions/parent-chat.jsonl"
              }
            ]"#;
        fixture.write_manifest("parent-chat.json", running);

        let first = subagent_sessions_for(
            &fixture.store,
            &fixture.data_dir(),
            fixture.workspace_id,
            fixture.parent.id,
        )
        .expect("first listing");
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].status, "running");
        assert_eq!(first[0].ended_at, None);
        assert_eq!(first[1].status, "done");
        assert_eq!(first[1].ended_at.as_deref(), Some("2026-07-01T10:04:00Z"));
        let ids: Vec<i64> = first.iter().map(|child| child.session_id).collect();

        // The same manifest again: same rows, no duplicates, no drift.
        let again = subagent_sessions_for(
            &fixture.store,
            &fixture.data_dir(),
            fixture.workspace_id,
            fixture.parent.id,
        )
        .expect("second listing");
        assert_eq!(
            again.iter().map(|child| child.session_id).collect::<Vec<_>>(),
            ids,
            "re-reading a manifest must not add rows"
        );

        // The child finishes: the very next call says so, with no write of ours.
        fixture.write_manifest(
            "parent-chat.json",
            &running.replace("\"running\"", "\"done\""),
        );
        let finished = subagent_sessions_for(
            &fixture.store,
            &fixture.data_dir(),
            fixture.workspace_id,
            fixture.parent.id,
        )
        .expect("third listing");
        assert_eq!(finished[0].session_id, ids[0]);
        assert_eq!(finished[0].status, "done");
        assert_eq!(
            fixture
                .store
                .child_sessions(fixture.parent.id)
                .expect("children")
                .len(),
            2,
            "two children, still"
        );
    }

    // S2 of `drawer-responsiveness-oauth.md`: one listing is one scan of the
    // extension's directory. The indexed rows and the live status come from the
    // same snapshot, so the sidebar refreshing many chats cannot pay for the same
    // disk walk twice per query.
    #[test]
    fn subagent_sessions_scans_the_manifests_once_per_query() {
        let fixture = SubagentFixture::new("subagent_single_scan");
        fixture.write_manifest(
            "parent-chat.json",
            r#"[
              {
                "name": "review",
                "agent": "reviewer",
                "sessionPath": "/data/sessions/subagents/child-a.jsonl",
                "status": "running",
                "startedAt": "2026-07-01T10:00:00Z",
                "endedAt": null,
                "parentSessionPath": "/sessions/parent-chat.jsonl"
              }
            ]"#,
        );
        let before = manifest_scan_count();

        let children = subagent_sessions_for(
            &fixture.store,
            &fixture.data_dir(),
            fixture.workspace_id,
            fixture.parent.id,
        )
        .expect("list children");

        // The row and its live status really were read from that one snapshot.
        assert_eq!(children.len(), 1, "got {children:?}");
        assert_eq!(children[0].agent, "reviewer");
        assert_eq!(children[0].status, "running");
        assert_eq!(
            manifest_scan_count() - before,
            1,
            "one query must walk the manifest directory once"
        );
    }

    // H3.6 negative case: a manifest whose parent path matches no indexed session
    // links nothing. Attaching the child to "the workspace's active chat" would
    // be a guess, and a wrong guess shows a stranger's work under this turn.
    #[test]
    fn subagent_sessions_skips_a_manifest_with_no_indexed_parent() {
        let fixture = SubagentFixture::new("subagent_orphan");
        fixture.write_manifest(
            "orphan.json",
            r#"[
              {
                "name": "review",
                "agent": "reviewer",
                "sessionPath": "/data/sessions/subagents/orphan.jsonl",
                "status": "done",
                "startedAt": "2026-07-01T10:00:00Z",
                "endedAt": "2026-07-01T10:02:00Z",
                "parentSessionPath": "/sessions/not-indexed.jsonl"
              }
            ]"#,
        );

        let children = subagent_sessions_for(
            &fixture.store,
            &fixture.data_dir(),
            fixture.workspace_id,
            fixture.parent.id,
        )
        .expect("an orphan manifest is not an error");
        assert!(children.is_empty(), "got {children:?}");
        assert_eq!(
            fixture.store.list_sessions(fixture.workspace_id).expect("list"),
            vec![fixture.parent.clone()],
            "nothing was indexed for a manifest the host cannot attribute"
        );

        // A parent that is not indexed at all is answered by name.
        let err = subagent_sessions_for(
            &fixture.store,
            &fixture.data_dir(),
            fixture.workspace_id,
            424242,
        )
        .expect_err("an unindexed parent");
        assert!(err.contains("unknown session"), "got {err:?}");
    }

    // H3.4: a child's file is a normal pi JSONL, so the existing reader serves the
    // child view without a special case — no new endpoint, no copy of the content.
    #[test]
    fn a_child_transcript_reads_like_any_other_session() {
        let fixture = SubagentFixture::new("subagent_transcript");
        let session_dir = fixture.data_dir().join("sessions").join("subagents");
        std::fs::create_dir_all(&session_dir).expect("session dir");
        let child_path = session_dir.join("child-a.jsonl");
        std::fs::write(
            &child_path,
            "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"review the diff\"}}\n\
             {\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":\"nothing to fix\"}}\n",
        )
        .expect("child transcript");
        let manifest = format!(
            r#"[
              {{
                "name": "review",
                "agent": "reviewer",
                "sessionPath": "{}",
                "status": "done",
                "startedAt": "2026-07-01T10:00:00Z",
                "endedAt": "2026-07-01T10:02:00Z",
                "parentSessionPath": "{}"
              }}
            ]"#,
            child_path.to_string_lossy(),
            fixture.parent.pi_session_path
        );
        fixture.write_manifest("parent-chat.json", &manifest);

        let children = subagent_sessions_for(
            &fixture.store,
            &fixture.data_dir(),
            fixture.workspace_id,
            fixture.parent.id,
        )
        .expect("list children");
        let transcript = session_transcript_for(
            &fixture.store,
            &children[0].session_id.to_string(),
        )
        .expect("the child transcript");

        assert_eq!(transcript.len(), 2, "got {transcript:?}");
        assert_eq!(transcript[0].role, "user");
        assert_eq!(transcript[0].text, "review the diff");
        assert_eq!(transcript[1].role, "assistant");
        assert_eq!(transcript[1].text, "nothing to fix");
    }

    // ── S1-S2 of `composer-image-input.md`: prompts that carry images ─────────

    /// The paths the image preflight reads, rooted in the fixture's own home.
    fn image_paths(
        fixture: &SessionFixture,
    ) -> (
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let home = fixture.path("home");
        (
            fixture.config_dir(),
            models_file(&home),
            catalog_file(&home),
            credentials_file(&home),
        )
    }

    /// S2: the image-only turn reaches the child as pi's own `ImageContent`, and
    /// the session it opens is titled from what it carried (there is no text to
    /// derive a title from).
    #[cfg(unix)]
    #[test]
    fn send_prompt_carries_the_images_pi_expects() {
        let fixture = SessionFixture::new("prompt_images");
        let script = fixture.write_image_pi("fake-pi.sh", r#"["text","image"]"#);
        fixture.write_settings(&script);
        let row = fixture.add_session("s1.jsonl", true, "");
        let runtime = lifecycle_runtime();
        let key = row.id.to_string();
        block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &registered_gate(),
            &key,
        ))
        .expect("open session");

        let (config_dir, models, catalog, credentials) = image_paths(&fixture);
        let paths = ImageCapabilityPaths {
            config_dir: &config_dir,
            models: &models,
            catalog: &catalog,
            credentials: &credentials,
        };
        send_prompt_for(
            &fixture.store,
            &paths,
            &runtime,
            fixture.workspace_id,
            row.id,
            String::new(),
            vec![PromptImage {
                mime_type: "image/jpg".to_string(),
                data: "Zm9v".to_string(),
            }],
        )
        .expect("send the image-only prompt");

        let sent = recorded_stdin(&fixture, "fake-pi.sh");
        let request = sent
            .iter()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("json line"))
            .find(|value| value["type"] == "prompt")
            .expect("the child must receive the prompt");
        assert_eq!(request["message"], "", "an image-only turn has no text");
        assert_eq!(
            request["images"],
            serde_json::json!([{"type": "image", "data": "Zm9v", "mimeType": "image/jpeg"}]),
            "got: {request}"
        );
        assert_eq!(
            fixture
                .store
                .session_by_id(row.id)
                .expect("session row")
                .expect("row")
                .title
                .as_deref(),
            Some("Image"),
            "an image-only first prompt still names the session"
        );

        runtime.stop_all();
    }

    // S1-S2: a text-only prompt is the same protocol line this path sent before
    // images existed — no empty `images` key, so nothing about it changed.
    #[cfg(unix)]
    #[test]
    fn send_prompt_keeps_the_text_only_wire_line_unchanged() {
        let fixture = SessionFixture::new("prompt_text_only");
        let script = fixture.write_image_pi("fake-pi.sh", r#"["text","image"]"#);
        fixture.write_settings(&script);
        let row = fixture.add_session("s1.jsonl", true, "");
        let runtime = lifecycle_runtime();
        let key = row.id.to_string();
        block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &registered_gate(),
            &key,
        ))
        .expect("open session");

        let (config_dir, models, catalog, credentials) = image_paths(&fixture);
        let paths = ImageCapabilityPaths {
            config_dir: &config_dir,
            models: &models,
            catalog: &catalog,
            credentials: &credentials,
        };
        send_prompt_for(
            &fixture.store,
            &paths,
            &runtime,
            fixture.workspace_id,
            row.id,
            "Fix the drawer".to_string(),
            Vec::new(),
        )
        .expect("send the text prompt");

        let sent = recorded_stdin(&fixture, "fake-pi.sh");
        let request = sent
            .iter()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("json line"))
            .find(|value| value["type"] == "prompt")
            .expect("the child must receive the prompt");
        assert_eq!(request["message"], "Fix the drawer");
        assert!(
            request.get("images").is_none(),
            "a text-only prompt carries no images key: {request}"
        );

        runtime.stop_all();
    }

    // S2: the live model is asked, and a model that declares text only is a
    // refusal *before* the prompt leaves — no silent model switch, no dropped
    // image. The session keeps its untitled row: nothing was sent.
    #[cfg(unix)]
    #[test]
    fn send_prompt_refuses_images_for_a_text_only_model() {
        let fixture = SessionFixture::new("prompt_images_refused");
        let script = fixture.write_image_pi("fake-pi.sh", r#"["text"]"#);
        fixture.write_settings(&script);
        let row = fixture.add_session("s1.jsonl", true, "");
        let runtime = lifecycle_runtime();
        let key = row.id.to_string();
        block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &registered_gate(),
            &key,
        ))
        .expect("open session");

        let (config_dir, models, catalog, credentials) = image_paths(&fixture);
        let paths = ImageCapabilityPaths {
            config_dir: &config_dir,
            models: &models,
            catalog: &catalog,
            credentials: &credentials,
        };
        let err = send_prompt_for(
            &fixture.store,
            &paths,
            &runtime,
            fixture.workspace_id,
            row.id,
            "look".to_string(),
            vec![PromptImage {
                mime_type: "image/png".to_string(),
                data: "Zm9v".to_string(),
            }],
        )
        .expect_err("a text-only model refuses the image");
        assert!(
            err.contains("does not accept image input"),
            "the refusal explains the capability, not a protocol error: {err}"
        );

        let sent = recorded_stdin(&fixture, "fake-pi.sh");
        assert!(
            !sent.iter().any(|line| line.contains("\"type\":\"prompt\"")),
            "nothing reached the child as a prompt: {sent:?}"
        );
        assert_eq!(
            fixture
                .store
                .session_by_id(row.id)
                .expect("session row")
                .expect("row")
                .title,
            None,
            "a refused prompt does not title the session"
        );

        runtime.stop_all();
    }

    // S2: the bounds are validated before anything is sent or titled, and each
    // rejection names its own reason. No session is needed: the validation comes
    // first, which is what keeps a bad payload away from the child.
    #[test]
    fn send_prompt_rejects_an_empty_turn_and_malformed_images() {
        let fixture = SessionFixture::new("prompt_images_bounds");
        let (config_dir, models, catalog, credentials) = image_paths(&fixture);
        let paths = ImageCapabilityPaths {
            config_dir: &config_dir,
            models: &models,
            catalog: &catalog,
            credentials: &credentials,
        };
        let runtime = lifecycle_runtime();
        let key = "999";

        let empty = send_prompt_for(
            &fixture.store,
            &paths,
            &runtime,
            fixture.workspace_id,
            key.parse().expect("id"),
            "   ".to_string(),
            Vec::new(),
        )
        .expect_err("an empty turn is refused");
        assert_eq!(empty, "prompt text is empty");

        let too_many = vec![
            PromptImage {
                mime_type: "image/png".to_string(),
                data: "Zm9v".to_string(),
            };
            image_input::MAX_PROMPT_IMAGES + 1
        ];
        let err = send_prompt_for(
            &fixture.store,
            &paths,
            &runtime,
            fixture.workspace_id,
            key.parse().expect("id"),
            "look".to_string(),
            too_many,
        )
        .expect_err("too many images are refused");
        assert!(err.contains("at most 4 images"), "{err}");

        for (mime, data, expected) in [
            ("text/plain", "Zm9v", "unsupported image format"),
            ("image/png", "not base64!", "invalid image data"),
            ("image/png", "", "invalid image data"),
        ] {
            let err = send_prompt_for(
                &fixture.store,
                &paths,
                &runtime,
                fixture.workspace_id,
                key.parse().expect("id"),
                "look".to_string(),
                vec![PromptImage {
                    mime_type: mime.to_string(),
                    data: data.to_string(),
                }],
            )
            .expect_err("a malformed image is refused");
            assert!(err.contains(expected), "{mime:?}/{data:?}: {err}");
        }
    }

    // S2: with no live child (or a pi that does not report its model), the answer
    // comes from pi's own catalogue. A model the catalogue does not describe is
    // not guessed: the caller refuses instead.
    #[test]
    fn image_capability_falls_back_to_pis_catalogue() {
        let fixture = SessionFixture::new("image_catalogue");
        let (config_dir, models, catalog, credentials) = image_paths(&fixture);
        std::fs::create_dir_all(models.parent().expect("models parent"))
            .expect("create pi agent dir");
        std::fs::write(
            &models,
            r#"{"providers":{"nan":{"name":"NaN","baseUrl":"https://nan.test/v1","apiKey":"sk-nan","models":[{"id":"vision","name":"Vision","input":["text","image"]},{"id":"plain","name":"Plain","input":["text"]}]}}}"#,
        )
        .expect("write models.json");
        settings::save(
            &settings_file(&config_dir),
            &Settings {
                provider: "nan".to_string(),
                model: "vision".to_string(),
                ..Settings::default()
            },
        )
        .expect("save settings");
        let paths = ImageCapabilityPaths {
            config_dir: &config_dir,
            models: &models,
            catalog: &catalog,
            credentials: &credentials,
        };

        assert_eq!(
            catalogue_model_accepts_images(&paths, "nan", "vision"),
            Some(true)
        );
        assert_eq!(
            catalogue_model_accepts_images(&paths, "nan", "plain"),
            Some(false)
        );
        assert_eq!(
            catalogue_model_accepts_images(&paths, "nan", "missing"),
            None,
            "a model the catalogue does not describe is not guessed"
        );
        assert_eq!(catalogue_model_accepts_images(&paths, "", "vision"), None);
    }
}
