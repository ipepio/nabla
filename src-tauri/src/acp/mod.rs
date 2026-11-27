//! ACP (Agent Client Protocol) process supervisor for Nabla with bidirectional
//! session continuity.
//!
//! - Injects Nabla's embedded HTTP MCP server (`nabla-browser`: Browser,
//!   Terminal, Document, Web Search, Memory, Session Context, Subagents) on
//!   `session/new`.
//! - Transfers prior Nabla conversation turns into the ACP agent on its first
//!   turn (`<conversation_history>`), and exposes the full transcript via the
//!   `session_context` MCP tool.
//! - Persists every ACP turn in Pi's native v3 JSONL tree format so switching
//!   back to any core Nabla model reloads the entire conversation seamlessly.
//! - Supports live `session/set_model` switching inside the same ACP session.

pub mod catalog;
pub mod protocol;
pub mod transcript;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use crate::mcp_server::Endpoint;
use crate::process::EventSink;
use crate::rpc::{AgentEvent, RuntimeStatus, StopReason, UiAnswer};

const SYSTEM_PROMPT_CONTEXT: &str = include_str!("../../../resources/nabla-system-prompt.md");
const STDERR_TAIL_LINES: usize = 20;
const INIT_TIMEOUT: Duration = Duration::from_secs(60);
const HANDOFF_MAX_TURNS: usize = 16;
const HANDOFF_MAX_CHARS: usize = 24_000;

/// Snapshot of a live ACP process, matching `ProcessManager::status`.
#[derive(Debug, Clone)]
pub struct AcpStatusSnapshot {
    pub status: RuntimeStatus,
    pub pid: u32,
    pub error: Option<String>,
}

struct PendingPermission {
    jsonrpc_id: Value,
    options: Vec<(String, String, String)>,
}

struct AcpSessionHandle {
    agent_id: String,
    model_id: Mutex<String>,
    pid: u32,
    acp_session_id: Mutex<Option<String>>,
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    status: Mutex<RuntimeStatus>,
    last_error: Mutex<Option<String>>,
    stderr_tail: Mutex<VecDeque<String>>,
    next_id: AtomicU64,
    first_turn: AtomicBool,
    pending_rpc: Mutex<HashMap<u64, mpsc::Sender<Result<Value, String>>>>,
    pending_permissions: Mutex<HashMap<String, PendingPermission>>,
    assistant_buf: Mutex<String>,
    cwd: PathBuf,
    transcript_path: PathBuf,
    data_dir: PathBuf,
    sink: EventSink,
    session_key: String,
    stop_requested: AtomicBool,
}

impl AcpSessionHandle {
    fn write_json(&self, value: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(value)
            .map_err(|e| format!("failed to encode ACP JSON-RPC frame: {e}"))?;
        line.push('\n');
        let mut guard = self
            .stdin
            .lock()
            .map_err(|_| "ACP stdin lock poisoned".to_string())?;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| "ACP process stdin is closed".to_string())?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("failed to write to ACP stdin: {e}"))
    }

    fn send_rpc_request(
        &self,
        id: u64,
        payload: &Value,
        timeout: Option<Duration>,
    ) -> Result<Value, String> {
        let (tx, rx) = mpsc::channel();
        {
            let mut map = self
                .pending_rpc
                .lock()
                .map_err(|_| "ACP pending_rpc lock poisoned".to_string())?;
            map.insert(id, tx);
        }
        if let Err(err) = self.write_json(payload) {
            if let Ok(mut map) = self.pending_rpc.lock() {
                map.remove(&id);
            }
            return Err(err);
        }
        match timeout {
            Some(dur) => rx.recv_timeout(dur).map_err(|_| {
                if let Ok(mut map) = self.pending_rpc.lock() {
                    map.remove(&id);
                }
                format!("ACP request {id} timed out after {}s", dur.as_secs())
            })?,
            None => rx
                .recv()
                .map_err(|_| "ACP process exited before answering request".to_string())?,
        }
    }

    fn set_status(&self, new_status: RuntimeStatus, error: Option<String>) {
        if let Ok(mut s) = self.status.lock() {
            *s = new_status;
        }
        if let Some(ref e) = error
            && let Ok(mut le) = self.last_error.lock()
        {
            *le = Some(e.clone());
        }
        (self.sink)(
            &self.session_key,
            &AgentEvent::ProcessStatus {
                status: new_status,
                error,
            },
        );
    }

    fn read_permission_mode(&self) -> String {
        let path = self.data_dir.join("permissions.json");
        fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|v| v.get("mode").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| "write".to_string())
    }

    fn handle_permission_request(&self, jsonrpc_id: Value, params: &Value) {
        let (title, message, options) = protocol::parse_permission_request(params);
        let mode = self.read_permission_mode();

        if mode == "yolo" {
            let chosen = protocol::pick_auto_allow_option(&options);
            let resp = protocol::build_permission_response(&jsonrpc_id, chosen.as_deref());
            let _ = self.write_json(&resp);
            return;
        }

        let req_id = format!("acp-perm-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let labels: Vec<String> = options.iter().map(|(_, label, _)| label.clone()).collect();
        if labels.is_empty() {
            let resp = protocol::build_permission_response(&jsonrpc_id, None);
            let _ = self.write_json(&resp);
            return;
        }

        if let Ok(mut pending) = self.pending_permissions.lock() {
            pending.insert(
                req_id.clone(),
                PendingPermission {
                    jsonrpc_id,
                    options,
                },
            );
        }

        (self.sink)(
            &self.session_key,
            &AgentEvent::ExtensionUiRequest {
                id: req_id,
                method: "select".to_string(),
                title: Some(format!("Permission: {title}")),
                options: Some(labels),
                message,
                placeholder: None,
            },
        );
    }

    fn answer_permission(&self, request_id: &str, answer: &UiAnswer) -> Result<(), String> {
        let pending = {
            let mut map = self
                .pending_permissions
                .lock()
                .map_err(|_| "ACP permission lock poisoned".to_string())?;
            map.remove(request_id)
                .ok_or_else(|| format!("unknown ACP permission request: {request_id}"))?
        };
        let selected_option_id = match answer {
            UiAnswer::Value { value } => pending
                .options
                .iter()
                .find(|(id, label, _)| label == value || id == value)
                .map(|(id, _, _)| id.clone()),
            UiAnswer::Confirmed { confirmed: true } => {
                protocol::pick_auto_allow_option(&pending.options)
            }
            UiAnswer::Confirmed { confirmed: false } | UiAnswer::Cancelled { .. } => None,
        };
        let resp =
            protocol::build_permission_response(&pending.jsonrpc_id, selected_option_id.as_deref());
        self.write_json(&resp)
    }

    fn wait_until_session_ready(&self) -> Result<String, String> {
        for _ in 0..200 {
            if let Ok(guard) = self.acp_session_id.lock()
                && let Some(ref sid) = *guard
            {
                return Ok(sid.clone());
            }
            let st = self
                .status
                .lock()
                .map(|s| *s)
                .unwrap_or(RuntimeStatus::Stopped);
            if matches!(st, RuntimeStatus::Crashed | RuntimeStatus::Stopped) {
                let err = self
                    .last_error
                    .lock()
                    .ok()
                    .and_then(|e| e.clone())
                    .unwrap_or_else(|| format!("ACP agent {} failed to start", self.agent_id));
                return Err(err);
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(format!(
            "{} is still starting up — please retry in a moment",
            self.agent_id
        ))
    }

    fn apply_model(&self, model_id: &str) -> Result<(), String> {
        if model_id.trim().is_empty() || model_id == "default" {
            return Ok(());
        }
        let sid = self.wait_until_session_ready()?;
        let req_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = protocol::build_session_set_model_request(req_id, &sid, model_id);
        let _ = self.send_rpc_request(req_id, &req, Some(Duration::from_secs(15)));
        if let Ok(mut m) = self.model_id.lock() {
            *m = model_id.to_string();
        }
        Ok(())
    }

    fn kill(&self) {
        self.stop_requested.store(true, Ordering::SeqCst);
        if let Ok(mut stdin) = self.stdin.lock() {
            *stdin = None;
        }
        if let Ok(mut child_guard) = self.child.lock()
            && let Some(mut child) = child_guard.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.set_status(RuntimeStatus::Stopped, None);
    }
}

/// Manages all live ACP child processes for the Nabla host.
#[derive(Clone)]
pub struct AcpManager {
    sessions: Arc<Mutex<HashMap<String, Arc<AcpSessionHandle>>>>,
    /// Tracks sessions that had turns written by ACP since the core engine last
    /// loaded their JSONL file, so returning to a core model reloads the file.
    dirty_for_core: Arc<Mutex<HashSet<String>>>,
    sink: EventSink,
    data_dir: PathBuf,
}

impl AcpManager {
    pub fn new(sink: EventSink, data_dir: PathBuf) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            dirty_for_core: Arc::new(Mutex::new(HashSet::new())),
            sink,
            data_dir,
        }
    }

    pub fn has_session(&self, session_key: &str) -> bool {
        self.sessions
            .lock()
            .map(|m| m.contains_key(session_key))
            .unwrap_or(false)
    }

    pub fn agent_id_for_session(&self, session_key: &str) -> Option<String> {
        self.sessions
            .lock()
            .ok()?
            .get(session_key)
            .map(|h| h.agent_id.clone())
    }

    /// Returns true and clears the dirty flag if ACP wrote turns to this
    /// session's JSONL since the core engine last read it.
    pub fn take_dirty_for_core(&self, session_key: &str) -> bool {
        self.dirty_for_core
            .lock()
            .map(|mut set| set.remove(session_key))
            .unwrap_or(false)
    }

    pub fn live_session_ids(&self) -> Vec<String> {
        self.sessions
            .lock()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn status(&self, session_key: &str) -> Option<AcpStatusSnapshot> {
        let handle = self.sessions.lock().ok()?.get(session_key).cloned()?;
        let status = *handle.status.lock().ok()?;
        let error = handle.last_error.lock().ok()?.clone();
        Some(AcpStatusSnapshot {
            status,
            pid: handle.pid,
            error,
        })
    }

    /// Switches the model inside an already-running ACP session via `session/set_model`.
    pub fn set_model(&self, session_key: &str, model_id: &str) -> Result<(), String> {
        let handle = self
            .sessions
            .lock()
            .map_err(|_| "ACP sessions lock poisoned".to_string())?
            .get(session_key)
            .cloned()
            .ok_or_else(|| format!("ACP session {session_key} is not running"))?;
        handle.apply_model(model_id)
    }

    /// Spawns and initializes an ACP child process (`initialize` + `session/new`
    /// + optional `session/set_model`) with Nabla's HTTP MCP server injected.
    pub fn spawn_session(
        &self,
        session_key: &str,
        agent_id: &str,
        model_id: &str,
        cwd: &Path,
        transcript_path: PathBuf,
        mcp: Option<&Endpoint>,
    ) -> Result<(), String> {
        if let Some(existing) = self
            .sessions
            .lock()
            .ok()
            .and_then(|m| m.get(session_key).cloned())
        {
            let st = existing
                .status
                .lock()
                .map(|s| *s)
                .unwrap_or(RuntimeStatus::Stopped);
            if existing.agent_id == agent_id
                && matches!(
                    st,
                    RuntimeStatus::Ready | RuntimeStatus::Running | RuntimeStatus::Spawning
                )
            {
                let current_model = existing
                    .model_id
                    .lock()
                    .map(|m| m.clone())
                    .unwrap_or_default();
                if current_model != model_id && !model_id.is_empty() && model_id != "default" {
                    let _ = existing.apply_model(model_id);
                }
                return Ok(());
            }
            self.stop_session(session_key);
        }

        let spec = catalog::resolve_launch(&self.data_dir, agent_id)?;
        let mut cmd = Command::new(&spec.command);
        cmd.args(&spec.args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        if let Some(cmd_dir) = Path::new(&spec.command)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            && !path_dirs.iter().any(|d| d == cmd_dir)
        {
            path_dirs.push(cmd_dir.to_path_buf());
        }
        for dir in catalog::extra_path_dirs() {
            if !path_dirs.iter().any(|d| d == &dir) {
                path_dirs.push(dir);
            }
        }
        if let Ok(joined) = std::env::join_paths(path_dirs) {
            cmd.env("PATH", joined);
        }

        let mut child = cmd.spawn().map_err(|e| {
            format!(
                "failed to spawn {} ({}): {e}",
                spec.name, spec.command
            )
        })?;
        let pid = child.id();
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ACP child stdin unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ACP child stdout unavailable".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "ACP child stderr unavailable".to_string())?;

        let handle = Arc::new(AcpSessionHandle {
            agent_id: spec.id.clone(),
            model_id: Mutex::new(model_id.to_string()),
            pid,
            acp_session_id: Mutex::new(None),
            stdin: Mutex::new(Some(stdin)),
            child: Mutex::new(Some(child)),
            status: Mutex::new(RuntimeStatus::Spawning),
            last_error: Mutex::new(None),
            stderr_tail: Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)),
            next_id: AtomicU64::new(1),
            first_turn: AtomicBool::new(true),
            pending_rpc: Mutex::new(HashMap::new()),
            pending_permissions: Mutex::new(HashMap::new()),
            assistant_buf: Mutex::new(String::new()),
            cwd: cwd.to_path_buf(),
            transcript_path,
            data_dir: self.data_dir.clone(),
            sink: self.sink.clone(),
            session_key: session_key.to_string(),
            stop_requested: AtomicBool::new(false),
        });

        handle.set_status(RuntimeStatus::Spawning, None);

        {
            let mut map = self
                .sessions
                .lock()
                .map_err(|_| "ACP sessions lock poisoned".to_string())?;
            map.insert(session_key.to_string(), handle.clone());
        }

        // Stderr reader thread.
        {
            let h = handle.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    if let Ok(mut tail) = h.stderr_tail.lock() {
                        if tail.len() >= STDERR_TAIL_LINES {
                            tail.pop_front();
                        }
                        tail.push_back(line);
                    }
                }
            });
        }

        // Stdout JSON-RPC reader thread.
        {
            let h = handle.clone();
            let sessions_map = self.sessions.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let Ok(msg) = serde_json::from_str::<Value>(trimmed) else {
                        continue;
                    };

                    if msg.get("method").is_none()
                        && let Some(id_num) = msg.get("id").and_then(Value::as_u64)
                    {
                        let sender = h
                            .pending_rpc
                            .lock()
                            .ok()
                            .and_then(|mut map| map.remove(&id_num));
                        if let Some(tx) = sender {
                            if let Some(err) = msg.get("error") {
                                let err_msg = err
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("ACP JSON-RPC error")
                                    .to_string();
                                let _ = tx.send(Err(err_msg));
                            } else {
                                let _ = tx
                                    .send(Ok(msg.get("result").cloned().unwrap_or(Value::Null)));
                            }
                        }
                        continue;
                    }

                    if let Some(method) = msg.get("method").and_then(Value::as_str) {
                        let params = msg.get("params").cloned().unwrap_or(Value::Null);
                        match method {
                            "session/update" => {
                                let (events, text_chunk) =
                                    protocol::translate_session_update(&h.session_key, &params);
                                if let Some(chunk) = text_chunk
                                    && let Ok(mut buf) = h.assistant_buf.lock()
                                {
                                    buf.push_str(&chunk);
                                }
                                for ev in events {
                                    (h.sink)(&h.session_key, &ev);
                                }
                            }
                            "session/request_permission" => {
                                if let Some(req_id) = msg.get("id").cloned() {
                                    h.handle_permission_request(req_id, &params);
                                }
                            }
                            _ => {}
                        }
                    }
                }

                let exit_code = h
                    .child
                    .lock()
                    .ok()
                    .and_then(|mut c| c.take())
                    .and_then(|mut child| child.wait().ok())
                    .and_then(|st| st.code());

                if let Ok(mut pending) = h.pending_rpc.lock() {
                    for (_, tx) in pending.drain() {
                        let _ = tx.send(Err("ACP process exited unexpectedly".to_string()));
                    }
                }

                let stderr_tail: Vec<String> = h
                    .stderr_tail
                    .lock()
                    .map(|q| q.iter().cloned().collect())
                    .unwrap_or_default();

                let crashed = !h.stop_requested.load(Ordering::SeqCst);
                if crashed {
                    let err_summary = stderr_tail
                        .last()
                        .cloned()
                        .unwrap_or_else(|| format!("ACP agent {} exited", h.agent_id));
                    h.set_status(RuntimeStatus::Crashed, Some(err_summary));
                } else {
                    h.set_status(RuntimeStatus::Stopped, None);
                }

                (h.sink)(
                    &h.session_key,
                    &AgentEvent::ProcessExit {
                        code: exit_code,
                        stderr_tail,
                    },
                );

                if let Ok(mut map) = sessions_map.lock() {
                    map.remove(&h.session_key);
                }
            });
        }

        // Handshake 1: initialize
        let init_id = handle.next_id.fetch_add(1, Ordering::Relaxed);
        let init_req = protocol::build_initialize_request(init_id);
        let init_result = match handle.send_rpc_request(init_id, &init_req, Some(INIT_TIMEOUT)) {
            Ok(v) => v,
            Err(err) => {
                handle.set_status(RuntimeStatus::Crashed, Some(err.clone()));
                handle.kill();
                return Err(format!("ACP initialize failed for {}: {err}", spec.name));
            }
        };

        // Handshake 2: session/new (with Nabla's HTTP MCP server injected)
        let try_session_new = |with_mcp: bool| -> Result<Value, String> {
            let new_id = handle.next_id.fetch_add(1, Ordering::Relaxed);
            let new_req = protocol::build_session_new_request(
                new_id,
                &cwd.to_string_lossy(),
                if with_mcp { mcp } else { None },
            );
            handle.send_rpc_request(new_id, &new_req, Some(INIT_TIMEOUT))
        };

        let session_result = match try_session_new(mcp.is_some()) {
            Ok(v) => v,
            Err(err) if err.to_ascii_lowercase().contains("auth") => {
                if let Some(method_id) = init_result
                    .get("authMethods")
                    .and_then(Value::as_array)
                    .and_then(|arr| arr.first())
                    .and_then(|m| m.get("id").and_then(Value::as_str))
                {
                    let auth_id = handle.next_id.fetch_add(1, Ordering::Relaxed);
                    let auth_req = protocol::build_authenticate_request(auth_id, method_id);
                    let _ = handle.send_rpc_request(auth_id, &auth_req, Some(INIT_TIMEOUT));
                }
                try_session_new(mcp.is_some())
                    .or_else(|_| try_session_new(false))
                    .map_err(|e2| {
                        handle.set_status(RuntimeStatus::Crashed, Some(e2.clone()));
                        handle.kill();
                        format!("{} requires sign-in (Settings -> Providers): {e2}", spec.name)
                    })?
            }
            Err(err) if mcp.is_some() => try_session_new(false).map_err(|e2| {
                handle.set_status(RuntimeStatus::Crashed, Some(e2.clone()));
                handle.kill();
                format!("ACP session/new failed for {}: {err} (retry: {e2})", spec.name)
            })?,
            Err(err) => {
                handle.set_status(RuntimeStatus::Crashed, Some(err.clone()));
                handle.kill();
                return Err(format!("ACP session/new failed for {}: {err}", spec.name));
            }
        };

        let acp_sid = session_result
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                handle.kill();
                "ACP session/new response missing sessionId".to_string()
            })?
            .to_string();

        if let Ok(mut guard) = handle.acp_session_id.lock() {
            *guard = Some(acp_sid);
        }

        // Apply the requested model inside this ACP session if not "default".
        if !model_id.trim().is_empty() && model_id != "default" {
            let _ = handle.apply_model(model_id);
        }

        handle.set_status(RuntimeStatus::Ready, None);
        Ok(())
    }

    /// Sends a user prompt to a live ACP session, transferring prior session
    /// history on the first turn and persisting both sides in Pi v3 JSONL format.
    pub fn send_prompt(
        &self,
        session_key: &str,
        text: String,
        images: Vec<(String, String)>,
    ) -> Result<(), String> {
        let handle = self
            .sessions
            .lock()
            .map_err(|_| "ACP sessions lock poisoned".to_string())?
            .get(session_key)
            .cloned()
            .ok_or_else(|| format!("ACP session {session_key} is not running"))?;

        let acp_sid = handle.wait_until_session_ready()?;

        // On the first turn of this ACP child, read prior turns from the session's
        // JSONL BEFORE appending the new user turn so the agent gets full continuity.
        let is_first = handle.first_turn.swap(false, Ordering::SeqCst);
        let handoff_ctx = if is_first {
            let prior_turns = transcript::read_session_messages(&handle.transcript_path);
            transcript::format_handoff_context(&prior_turns, HANDOFF_MAX_TURNS, HANDOFF_MAX_CHARS)
        } else {
            None
        };
        let sys_ctx = if is_first {
            Some(SYSTEM_PROMPT_CONTEXT)
        } else {
            None
        };

        let provider_key = catalog::runtime_key(&handle.agent_id);
        let model_id = handle
            .model_id
            .lock()
            .map(|m| m.clone())
            .unwrap_or_else(|_| "default".to_string());

        let _ = transcript::append_pi_session_message(
            &handle.transcript_path,
            &handle.cwd,
            "user",
            &text,
            &provider_key,
            &model_id,
        );
        if let Ok(mut dirty) = self.dirty_for_core.lock() {
            dirty.insert(session_key.to_string());
        }

        if let Ok(mut buf) = handle.assistant_buf.lock() {
            buf.clear();
        }

        let req_id = handle.next_id.fetch_add(1, Ordering::Relaxed);
        let req = protocol::build_session_prompt_request(
            req_id,
            &acp_sid,
            &text,
            &images,
            sys_ctx,
            handoff_ctx.as_deref(),
        );

        handle.set_status(RuntimeStatus::Running, None);
        (handle.sink)(&handle.session_key, &AgentEvent::AgentStart);
        (handle.sink)(&handle.session_key, &AgentEvent::TurnStart);

        let dirty_ref = self.dirty_for_core.clone();
        let skey = session_key.to_string();
        thread::spawn(move || {
            let outcome = handle.send_rpc_request(req_id, &req, None);
            let final_text = handle
                .assistant_buf
                .lock()
                .map(|b| b.clone())
                .unwrap_or_default();
            if !final_text.trim().is_empty() {
                let _ = transcript::append_pi_session_message(
                    &handle.transcript_path,
                    &handle.cwd,
                    "assistant",
                    &final_text,
                    &provider_key,
                    &model_id,
                );
                if let Ok(mut dirty) = dirty_ref.lock() {
                    dirty.insert(skey);
                }
            }

            let (stop_reason, error_message) = match outcome {
                Ok(res) => {
                    let sr = protocol::map_stop_reason(
                        res.get("stopReason").and_then(Value::as_str),
                    );
                    (sr, None)
                }
                Err(err) => (StopReason::Error, Some(err)),
            };

            let assistant_msg = json!({
                "role": "assistant",
                "content": [{ "type": "text", "text": final_text }],
                "stopReason": stop_reason.as_wire(),
            });

            (handle.sink)(
                &handle.session_key,
                &AgentEvent::TurnEnd {
                    message: assistant_msg.clone(),
                    tool_results: Vec::new(),
                    stop_reason: Some(stop_reason),
                    error_message,
                },
            );
            (handle.sink)(
                &handle.session_key,
                &AgentEvent::AgentEnd {
                    messages: vec![assistant_msg],
                },
            );
            if !handle.stop_requested.load(Ordering::SeqCst) {
                handle.set_status(RuntimeStatus::Ready, None);
            }
        });

        Ok(())
    }

    pub fn answer_ui_request(
        &self,
        session_key: &str,
        request_id: &str,
        answer: &UiAnswer,
    ) -> Result<(), String> {
        let handle = self
            .sessions
            .lock()
            .map_err(|_| "ACP sessions lock poisoned".to_string())?
            .get(session_key)
            .cloned()
            .ok_or_else(|| format!("ACP session {session_key} is not running"))?;
        handle.answer_permission(request_id, answer)
    }

    pub fn cancel_turn(&self, session_key: &str) -> Result<(), String> {
        let handle = self
            .sessions
            .lock()
            .map_err(|_| "ACP sessions lock poisoned".to_string())?
            .get(session_key)
            .cloned()
            .ok_or_else(|| format!("ACP session {session_key} is not running"))?;
        let acp_sid = handle.acp_session_id.lock().ok().and_then(|g| g.clone());
        if let Some(sid) = acp_sid {
            let notif = protocol::build_session_cancel_notification(&sid);
            let _ = handle.write_json(&notif);
        }
        Ok(())
    }

    pub fn stop_session(&self, session_key: &str) {
        if let Ok(mut map) = self.sessions.lock()
            && let Some(handle) = map.remove(session_key)
        {
            handle.kill();
        }
    }

    pub fn stop_all(&self) {
        if let Ok(mut map) = self.sessions.lock() {
            for (_, handle) in map.drain() {
                handle.kill();
            }
        }
    }
}
