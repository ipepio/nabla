// Command layer: the only seam between the webview and Rust (S4).
// Rule: "Vue never talks to pi directly" — every read or write crosses this
// module, which reads and writes the store/settings owned by the app. Payloads
// are camelCase so the TS contracts (T6) can mirror them field by field.

use std::path::{Path, PathBuf};

use tauri::State;

use crate::process::{ProcessManager, StartupOutcome};
use crate::rpc::{AgentEvent, RuntimeStatus};
use crate::settings::{self, Settings};
use crate::store::{SessionNew, SessionRow, Store, StoreResult, Workspace};

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

    let report = state
        .runtime
        .spawn(
            &session_id,
            &settings.pi_binary,
            &pi_session_path,
            &settings.provider,
            &settings.model,
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
fn open_session_core(
    store: &Store,
    runtime: &ProcessManager,
    config_dir: &Path,
    session_id: &str,
) -> Result<SessionDto, String> {
    // The index row is the runtime key (S1): resolving it first means an id that
    // is not indexed can never reach the spawn path.
    let row = session_row_by_id(store, session_id)?;

    // S5: a live process for this key is reused untouched — its pid comes from
    // the registry, never from a second spawn.
    if let Some(snapshot) = runtime.status(session_id) {
        return Ok(session_dto(&row, snapshot.pid, snapshot.status));
    }

    let workspace = workspace_by_id(store, row.workspace_id)?;
    // The engine path, provider and model are user-configured, so they are read
    // per open, not cached (S7).
    let settings = settings::load(&settings_file(config_dir)).map_err(display_err)?;
    let report = match runtime.spawn(
        session_id,
        &settings.pi_binary,
        // S3: the SAME file the index points at, so pi resumes that transcript
        // instead of creating another one. A file pi has not written yet is not
        // an error: pi creates it exactly there, keeping index and file equal.
        Path::new(&row.pi_session_path),
        &settings.provider,
        &settings.model,
        Path::new(&workspace.path),
    ) {
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
#[tauri::command]
pub fn open_session(state: State<'_, AppState>, session_id: String) -> Result<SessionDto, String> {
    open_session_core(
        &state.store,
        &state.runtime,
        &state.config_dir,
        &session_id,
    )
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

        let first = open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &key,
        )
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

        let second = open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &key,
        )
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

        let dto = open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            &row.id.to_string(),
        )
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

        let err = open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            "424242",
        )
        .expect_err("an unindexed id cannot be opened");
        assert!(
            err.contains("unknown session") && err.contains("424242"),
            "the error names the id, got {err:?}"
        );
        // A non-numeric id is rejected before anything is spawned too.
        let non_numeric = open_session_core(
            &fixture.store,
            &runtime,
            &fixture.config_dir(),
            "not-a-number",
        )
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
}
