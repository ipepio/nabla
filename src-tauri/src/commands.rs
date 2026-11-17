// Command layer: the only seam between the webview and Rust (S4).
// Rule: "Vue never talks to pi directly" — every read or write crosses this
// module, which reads and writes the store/settings owned by the app. Payloads
// are camelCase so the TS contracts (T6) can mirror them field by field.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tauri::State;
use tauri_plugin_opener::OpenerExt;

use crate::cli::{self, CliSessions, CliSink, PiAuthStatus, PiCliResult};
use crate::mcp::{self, McpAddDraft, McpScope, McpServerSummary};
use crate::process::{ProcessManager, StartupOutcome};
use crate::providers::{
    ModelInfo as ProviderModelInfo, ModelOption, ProviderDraft, ProviderModelDraft, ProviderModels,
    ProviderSummary,
};
use crate::rpc::{AgentEvent, RuntimeStatus};
use crate::settings::{self, Settings};
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
}

// Errors cross the bridge as plain strings for now: StoreError and SettingsError
// are mapped through Display. Typed error payloads, with a matching TS contract,
// arrive together with the runtime feature; nothing here freezes an error shape.
fn display_err<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

/// Path of the user-editable preferences file inside a config directory.
fn settings_file(config_dir: &Path) -> PathBuf {
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
    crate::settings::load(&path).map_err(display_err)
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
    let secs = now_millis() / 1000;
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

/// Spawns one headless pi for this workspace and registers it in the session
/// index (S1, S3, S7). The returned `sessionId` is the index row id in decimal
/// form — the key every later session command uses.
///
/// `async` on purpose: creating the process costs seconds of readiness wait, and
/// a synchronous command would hold Tauri's IPC thread for all of it. The
/// directory, the index row and the settings stay on the caller's thread; only
/// the spawn runs on a worker.
#[tauri::command]
pub async fn start_session(
    state: State<'_, AppState>,
    workspace_id: i64,
) -> Result<SessionDto, String> {
    // A workspace is a directory on disk: spawning pi somewhere else would
    // produce a session the user never asked for.
    let workspace = workspace_by_id(&state.store, workspace_id)?;

    // The engine path, provider and model are user-configured, so they are read
    // per start, not cached (S7).
    let settings = settings::load(&settings_path(&state)).map_err(display_err)?;

    let sessions_dir = state.data_dir.join(SESSIONS_DIR);
    std::fs::create_dir_all(&sessions_dir).map_err(display_err)?;
    let pi_session_path = sessions_dir.join(format!(
        "{}.jsonl",
        session_file_stem(workspace_id, now_millis())
    ));
    let pi_session_path_string = pi_session_path.to_string_lossy().to_string();

    // Index first: the row id is the runtime key, and no transcript is ever
    // stored here — only the path pi owns (S7).
    let timestamp = now_iso();
    let row = state
        .store
        .upsert_session(&SessionNew {
            workspace_id,
            pi_session_path: pi_session_path_string.clone(),
            title: None,
            model: None,
            started_at: timestamp.clone(),
            updated_at: timestamp,
        })
        .map_err(display_err)?;
    let session_id = row.id.to_string();

    // The spawn waits for pi's readiness probe; the handle and the paths are
    // owned before the move so the closure is `'static`. `settings` is only
    // needed by the spawn, so it moves with them.
    let runtime = state.runtime.clone();
    let key = session_id.clone();
    let workspace_path = workspace.path.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        runtime.spawn(
            &key,
            &settings.pi_binary,
            &pi_session_path,
            &settings.provider,
            &settings.model,
            Path::new(&workspace_path),
        )
    })
    .await
    .map_err(display_err)?
    .map_err(display_err)?;

    if report.outcome != StartupOutcome::Ready {
        let detail = report.stderr_tail.join("\n");
        return Err(startup_failure_message(report.outcome, &detail));
    }
    Ok(SessionDto {
        session_id,
        workspace_id,
        pi_session_path: pi_session_path_string,
        pid: report.pid.unwrap_or(0),
        status: report.status,
    })
}

/// Sends a prompt to the session's pi process. The answer that matters is the
/// forwarded event stream (S6); this only reports that pi accepted the request.
#[tauri::command]
pub fn send_prompt(
    state: State<'_, AppState>,
    workspace_id: i64,
    session_id: i64,
    text: String,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("prompt text is empty".to_string());
    }
    ensure_session_in_workspace(&state.store, workspace_id, session_id)?;

    // The first prompt names the session: the drawer shows this alias instead of
    // a generic label, and the row keeps `title = None` until then. A failure
    // here must never block the prompt, so it is deliberately ignored.
    if let Ok(Some(row)) = state.store.session_by_id(session_id)
        && row.title.is_none()
    {
        let _ = state
            .store
            .set_session_title(session_id, &derive_session_title(&text));
    }

    let mut fields = serde_json::Map::new();
    fields.insert("message".to_string(), serde_json::Value::String(text));
    let response = state
        .runtime
        .send(&session_id.to_string(), "prompt", fields)
        .map_err(display_err)?;
    if response.success {
        // A sent prompt is activity: the drawer sorts by `updated_at`, so the
        // session in use must climb to the top instead of sinking by creation.
        let _ = state.store.touch_session(session_id);
        Ok(())
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "pi refused the prompt".to_string()))
    }
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

/// Stops the session's pi process, waiting for it to exit (S4).
#[tauri::command]
pub fn stop_session(
    state: State<'_, AppState>,
    workspace_id: i64,
    session_id: i64,
) -> Result<(), String> {
    ensure_session_in_workspace(&state.store, workspace_id, session_id)?;
    state
        .runtime
        .stop_and_wait(&session_id.to_string())
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

    // Best effort: `stop_and_wait` is idempotent, so a session that was never
    // started (or already stopped) is not an error — the row still has to go.
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
    let key = session_id.to_string();
    let pi_session_path = row.pi_session_path.clone();
    let workspace_path = workspace.path.clone();
    let spawned = tauri::async_runtime::spawn_blocking(move || {
        spawned_runtime.spawn(
            &key,
            &settings.pi_binary,
            // S3: the SAME file the index points at, so pi resumes that
            // transcript instead of creating another one. A file pi has not
            // written yet is not an error: pi creates it exactly there, keeping
            // index and file equal.
            Path::new(&pi_session_path),
            &settings.provider,
            &settings.model,
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
    let opened = open_session_core(&store, &runtime, &config_dir, &session_id).await?;
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

/// S5: ids with a live pi process, read from the runtime registry only. The
/// list paints per-session state; it never opens anything (S2).
#[tauri::command]
pub fn live_session_ids(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(state.runtime.live_ids())
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

/// Closes one view and forgets it. Idempotent: an unknown id answers `false`.
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

/// Closes every view and answers how many were closed. The app also does this on
/// exit, so no child webview outlives the window that hosts it.
#[tauri::command]
pub fn embed_close_all(state: State<'_, AppState>, app: tauri::AppHandle) -> usize {
    state.embeds.close_all(&app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
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
    const FAKE_SESSION_PI: &str = r#"#!/bin/sh
printf '%s\n' "$@" >> '@ARGS@'
printf 'attempt\n' >> '@ATTEMPTS@'
printf '%s\n' '{"type":"response","id":"__startup_probe__","command":"get_state","success":true,"data":{}}'
while IFS= read -r line; do
  :
done
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
            use std::os::unix::fs::PermissionsExt;
            let path = self.path(name);
            let body = FAKE_SESSION_PI
                .replace("@ARGS@", &path.with_extension("args").display().to_string())
                .replace("@ATTEMPTS@", &self.path("attempts").display().to_string());
            std::fs::write(&path, body).expect("write fake pi");
            let mut perms = std::fs::metadata(&path)
                .expect("stat fake pi")
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).expect("chmod fake pi");
            path
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
        }
    }

    /// A manager with a sink that throws events away: these tests assert on the
    /// registry and the argv the child actually received.
    fn lifecycle_runtime() -> ProcessManager {
        let sink: crate::process::EventSink = Arc::new(|_, _| {});
        ProcessManager::with_timeouts(sink, lifecycle_timeouts())
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

        let first = block_on(open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
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
}
