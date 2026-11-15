// Command layer: the only seam between the webview and Rust (S4).
// Rule: "Vue never talks to pi directly" — every read or write crosses this
// module, which reads and writes the store/settings owned by the app. Payloads
// are camelCase so the TS contracts (T6) can mirror them field by field.

use std::path::{Path, PathBuf};

use tauri::State;

use crate::process::{ProcessManager, StartupOutcome};
use crate::rpc::{AgentEvent, RuntimeStatus};
use crate::settings::{self, Settings};
use crate::store::{SessionNew, SessionRow, Store, Workspace};

/// State shared with every command: the SQLite store (app-owned state), the
/// config directory where settings.json lives (human-owned preferences, S2), the
/// app data directory that holds the pi-owned session files (S7), and the pi
/// process supervisor (S1, S3).
pub struct AppState {
    pub store: Store,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub runtime: ProcessManager,
}

// Errors cross the bridge as plain strings for now: StoreError and SettingsError
// are mapped through Display. Typed error payloads, with a matching TS contract,
// arrive together with the runtime feature; nothing here freezes an error shape.
fn display_err<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

/// Path of the user-editable preferences file inside the app config directory.
fn settings_path(state: &AppState) -> PathBuf {
    state.config_dir.join("settings.json")
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

#[tauri::command]
pub fn remove_workspace(state: State<'_, AppState>, id: i64) -> Result<bool, String> {
    state.store.remove_workspace(id).map_err(display_err)
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
#[tauri::command]
pub fn start_session(state: State<'_, AppState>, workspace_id: i64) -> Result<SessionDto, String> {
    // A workspace is a directory on disk: spawning pi somewhere else would
    // produce a session the user never asked for.
    let workspace = state
        .store
        .list_workspaces()
        .map_err(display_err)?
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
        .ok_or_else(|| format!("unknown workspace: {workspace_id}"))?;

    // The engine path is user-configured, so it is read per start, not cached.
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

    let report = state
        .runtime
        .spawn(
            &session_id,
            &settings.pi_binary,
            &pi_session_path,
            Path::new(&workspace.path),
        )
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

    let mut fields = serde_json::Map::new();
    fields.insert("message".to_string(), serde_json::Value::String(text));
    let response = state
        .runtime
        .send(&session_id.to_string(), "prompt", fields)
        .map_err(display_err)?;
    if response.success {
        Ok(())
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "pi refused the prompt".to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
