// Process module: one headless pi child process per session (spec S3), owned and
// supervised by the Nabla host (S1, S4, S5).
//
// Design notes:
// - Spawn is std::process based: stdin/stdout/stderr are piped and no terminal is
//   ever attached, so nothing visible reaches the user (S1).
// - One process per session, keyed by the session index row (S3).
// - stdout is decoded by `rpc::FrameCodec`; request/response correlation is by
//   request id (a defect of the Stark reference, which had none).
// - Startup is `Spawning -> Ready` behind a correlated probe, with exactly one
//   retry and only for a crash, never for a timeout (FaqFirebase rule).
// - Kill is cross-platform: a whole process group on Unix (`libc::kill(-pid)`),
//   `taskkill /T` on Windows (S5, fixes the Unix-only defect of Stark
//   `process_manager.rs:409`).
//
// Wire references (read-only clones):
// - /tmp/nabla-pi-desktops.vYixxw/FaqFirebase/src/main/pi-rpc-manager.ts
//   (lifecycle, probe, retry rule, stop/kill sequencing)
// - /tmp/nabla-pi-desktops.vYixxw/StarkInternationalAI/src-tauri/src/process_manager.rs
//   (spawn args, env allowlist, per-session map, synthetic process_exit)

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::rpc::{
    AgentEvent, Frame, FrameCodec, RpcResponse, RuntimeStatus, STARTUP_PROBE_COMMAND,
    STARTUP_PROBE_ID,
};

/// Startup attempts per `spawn()`: the initial try plus one retry, used ONLY
/// when pi crashes before becoming ready. A timeout is never retried: respawning
/// would just burn another full timeout.
/// Mirror: `pi-rpc-manager.ts`, `STARTUP_MAX_ATTEMPTS`.
pub const STARTUP_MAX_ATTEMPTS: usize = 2;

/// Cap on the retained stderr lines per session (S4 "cola de stderr").
/// Mirror: `process_manager.rs` keeps the last 10 stderr lines per session.
pub const STDERR_TAIL_LINES: usize = 20;

/// Timings, injectable so lifecycle tests do not wait out real minutes.
/// Mirror: `pi-rpc-manager.ts`, `PiStartupTimeouts`.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// Deadline for the readiness probe's correlated answer.
    pub ready: Duration,
    /// Resend interval of the readiness probe (pi does not read stdin until its
    /// session is bound, so early writes can race).
    pub probe_interval: Duration,
    /// Grace for a child to exit after stdin EOF before it is signalled.
    pub stop_grace: Duration,
    /// Grace after SIGTERM (or `taskkill /T`) before the force kill.
    pub kill_grace: Duration,
    /// Deadline for a request's correlated response.
    pub response: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            ready: Duration::from_secs(120),
            probe_interval: Duration::from_millis(750),
            stop_grace: Duration::from_millis(1500),
            kill_grace: Duration::from_millis(1000),
            response: Duration::from_secs(60),
        }
    }
}

/// Result of one `spawn()`: the startup outcome plus what the caller needs to
/// report it (pid, final status, captured stderr when it failed).
/// Mirror: `pi-rpc-manager.ts`, `spawnAndAwaitReady` outcomes
/// 'ready' | 'crashed' | 'timeout' | 'aborted'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupOutcome {
    Ready,
    Crashed,
    Timeout,
    // 'aborted' is the deliberate-teardown outcome of the reference state
    // machine (`pi-rpc-manager.ts` `abortStartup`). Nabla cannot tear a start
    // down yet: a session is only reachable by key once `spawn` returned it,
    // so this outcome stays in the contract without a producer until a
    // cancellable start exists.
    #[allow(dead_code)]
    Aborted,
}

#[derive(Debug, Clone)]
pub struct SpawnReport {
    pub outcome: StartupOutcome,
    pub pid: Option<u32>,
    pub status: RuntimeStatus,
    pub stderr_tail: Vec<String>,
}

/// Live snapshot of one session process (S4 restart/status surface).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnapshot {
    pub status: RuntimeStatus,
    pub pid: u32,
    /// Captured stderr, reported only once the process is `Crashed` — healthy
    /// pi runs log benign lines to stderr. Mirror: `pi-rpc-manager.ts` `getStatus`.
    pub error: Option<String>,
}

/// Transport failures between Nabla and the child. Plain strings cross the Tauri
/// bridge (see commands.rs); this enum keeps them typed inside Rust.
#[derive(Debug)]
pub enum ProcessError {
    AlreadyRunning(String),
    NotRunning(String),
    Transport(String),
    ResponseTimeout(String),
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessError::AlreadyRunning(key) => write!(f, "a pi process is already running for session {key}"),
            ProcessError::NotRunning(key) => write!(f, "no pi process is running for session {key}"),
            ProcessError::Transport(msg) => write!(f, "pi rpc transport error: {msg}"),
            ProcessError::ResponseTimeout(msg) => write!(f, "pi rpc response timeout: {msg}"),
        }
    }
}

impl std::error::Error for ProcessError {}

/// Where every forwarded event goes: the session key it belongs to plus the
/// event itself. The command layer turns this into the Tauri `agent-event`
/// envelope (S6); tests collect it in a vector.
pub type EventSink = Arc<dyn Fn(&str, &AgentEvent) + Send + Sync>;

/// One message into the child's stdin writer thread.
enum StdinMsg {
    /// A complete NDJSON line, newline included (the codec owns the framing).
    Line(String),
    /// Close stdin so the child sees EOF (the graceful half of a stop).
    Close,
}

/// Settles the startup attempt that is waiting for readiness.
enum StartupSignal {
    /// The readiness probe's correlated answer arrived.
    Ready,
    /// The child exited before becoming ready: a startup crash.
    Exited,
}

/// One-shot exit latch so any thread can wait for the child to be reaped
/// without holding the `Child` handle itself.
#[derive(Default)]
struct ExitSignal {
    code: Mutex<Option<Option<i32>>>,
    latched: Condvar,
}

impl ExitSignal {
    fn new() -> ExitSignal {
        ExitSignal::default()
    }

    /// True when the child has exited (already, or within `timeout`).
    fn wait(&self, timeout: Duration) -> bool {
        let guard = lock(&self.code);
        let (guard, _) = self
            .latched
            .wait_timeout_while(guard, timeout, |code| code.is_none())
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.is_some()
    }

    fn mark(&self, code: Option<i32>) {
        *lock(&self.code) = Some(code);
        self.latched.notify_all();
    }
}

/// Locks without panicking: a poisoned lock in a background thread must not turn
/// into a dead session the host cannot supervise.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Appends one diagnostic line, keeping the tail bounded (S4).
fn push_stderr(tail: &Mutex<VecDeque<String>>, line: String) {
    let mut guard = lock(tail);
    while guard.len() >= STDERR_TAIL_LINES {
        guard.pop_front();
    }
    guard.push_back(line);
}

fn tail_snapshot(tail: &Mutex<VecDeque<String>>) -> Vec<String> {
    lock(tail).iter().cloned().collect()
}

/// Publishes a lifecycle transition, emitting an event only when the status
/// actually changes so subscribers never see repeated identical states.
fn set_status(
    sink: &EventSink,
    key: &str,
    status: &Mutex<RuntimeStatus>,
    next: RuntimeStatus,
    error: Option<String>,
) -> RuntimeStatus {
    let changed = {
        let mut guard = lock(status);
        if *guard == next {
            false
        } else {
            *guard = next;
            true
        }
    };
    if changed {
        sink(key, &AgentEvent::ProcessStatus { status: next, error });
    }
    next
}

/// Routes one decoded frame: responses to their waiting request (or the startup
/// probe), events to the sink, undecodable lines to the stderr tail.
fn dispatch_frame(
    frame: Frame,
    key: &str,
    sink: &EventSink,
    pending: &Mutex<HashMap<String, mpsc::SyncSender<RpcResponse>>>,
    ready: &AtomicBool,
    startup: &mpsc::Sender<StartupSignal>,
    stderr_tail: &Mutex<VecDeque<String>>,
) {
    match frame {
        Frame::Response(response) => {
            if response.id.as_deref() == Some(STARTUP_PROBE_ID) {
                // Success or an "unknown command" error both prove the loop is
                // live: key off the correlated answer, not off stdout activity.
                ready.store(true, Ordering::SeqCst);
                let _ = startup.send(StartupSignal::Ready);
                return;
            }
            // Correlation is the only consumer of responses in Nabla: a stale
            // answer (timeout already fired) has nobody left to tell.
            let Some(id) = response.id.clone() else {
                return;
            };
            if let Some(waiter) = lock(pending).remove(&id) {
                let _ = waiter.send(response);
            }
        }
        Frame::Event(event) => sink(key, &stamp_session(event, key)),
        Frame::InvalidLine { reason, .. } => {
            push_stderr(stderr_tail, format!("undecodable pi stdout line: {reason}"));
        }
    }
}

/// Stamps the owning session onto events that need routing (R4 mirrors it).
fn stamp_session(mut event: AgentEvent, key: &str) -> AgentEvent {
    if let AgentEvent::MessageUpdate { session_id, .. } = &mut event {
        *session_id = Some(key.to_string());
    }
    event
}

/// Graceful-then-force stop of a whole process tree (S5).
/// `exit` is waited on first so an already-dead child costs nothing.
fn force_stop(pid: u32, exit: &ExitSignal, timeouts: &Timeouts) {
    // Grace for the child to leave on its own (stdin EOF, or the abort command).
    if exit.wait(timeouts.stop_grace) {
        return;
    }
    signal_terminate(pid);
    if exit.wait(timeouts.kill_grace) {
        return;
    }
    // A child that ignores the polite signal must not outlive the host (S1).
    signal_kill(pid);
    let _ = exit.wait(timeouts.kill_grace);
}

/// Supervisor of all per-session pi children.
///
/// Cheap to clone: every handle shares the same registry behind the `Arc`, so a
/// command can move one onto a worker thread and still address the children the
/// host already registered.
#[derive(Clone)]
pub struct ProcessManager {
    sink: EventSink,
    timeouts: Timeouts,
    sessions: Arc<Mutex<HashMap<String, Arc<SessionProcess>>>>,
}

impl ProcessManager {
    pub fn new(sink: EventSink) -> ProcessManager {
        ProcessManager::with_timeouts(sink, Timeouts::default())
    }

    pub fn with_timeouts(sink: EventSink, timeouts: Timeouts) -> ProcessManager {
        ProcessManager {
            sink,
            timeouts,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Spawns one headless pi for `key` and waits for readiness, with the single
    /// crash retry. `Err` means the spawn could not even be attempted (already
    /// running); a failed startup is reported inside `SpawnReport`.
    /// `provider`/`model` come from settings (S7) and are decided per spawn.
    pub fn spawn(
        &self,
        key: &str,
        pi_binary: &str,
        session_file: &Path,
        provider: &str,
        model: &str,
        cwd: &Path,
    ) -> Result<SpawnReport, ProcessError> {
        // S3: one process per session. Checked before spawning, not after, so a
        // second caller can never orphan a live child.
        if lock(&self.sessions).contains_key(key) {
            return Err(ProcessError::AlreadyRunning(key.to_string()));
        }
        let mut attempt = 1;
        loop {
            let report = self.spawn_once(key, pi_binary, session_file, provider, model, cwd);
            match report.outcome {
                // A crash before ready is often transient (an AV lock, a
                // momentary ENOENT): retry exactly once. A timeout is not
                // retried — respawning would just burn another full deadline.
                StartupOutcome::Crashed if attempt < STARTUP_MAX_ATTEMPTS => {
                    attempt += 1;
                }
                _ => return Ok(report),
            }
        }
    }

    /// One spawn attempt: start the child, wait for readiness, and either
    /// register the live session or reap what is left of it.
    fn spawn_once(
        &self,
        key: &str,
        pi_binary: &str,
        session_file: &Path,
        provider: &str,
        model: &str,
        cwd: &Path,
    ) -> SpawnReport {
        // Announce the attempt before anything can fail (S4).
        (self.sink)(
            key,
            &AgentEvent::ProcessStatus {
                status: RuntimeStatus::Spawning,
                error: None,
            },
        );
        let status = Arc::new(Mutex::new(RuntimeStatus::Spawning));
        let stopping = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(AtomicBool::new(false));
        let stderr_tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
        let exit = Arc::new(ExitSignal::new());
        let pending: Arc<Mutex<HashMap<String, mpsc::SyncSender<RpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let mut child = match build_command(pi_binary, session_file, provider, model, cwd).spawn() {
            Ok(child) => child,
            Err(err) => {
                let message = format!("failed to spawn pi ({pi_binary}): {err}");
                push_stderr(&stderr_tail, message.clone());
                set_status(
                    &self.sink,
                    key,
                    &status,
                    RuntimeStatus::Crashed,
                    Some(message),
                );
                return SpawnReport {
                    outcome: StartupOutcome::Crashed,
                    pid: None,
                    status: RuntimeStatus::Crashed,
                    stderr_tail: tail_snapshot(&stderr_tail),
                };
            }
        };
        let pid = child.id();
        let stdin_pipe = child.stdin.take();
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let (stdin_tx, stdin_rx) = mpsc::channel::<StdinMsg>();
        let (startup_tx, startup_rx) = mpsc::channel::<StartupSignal>();

        // stdin writer: one channel in, one pipe out, nothing else touches it.
        if let Some(mut pipe) = stdin_pipe {
            thread::spawn(move || {
                for msg in stdin_rx {
                    let StdinMsg::Line(line) = msg else {
                        break;
                    };
                    if pipe.write_all(line.as_bytes()).is_err() || pipe.flush().is_err() {
                        break;
                    }
                }
                // Dropping the pipe closes pi's stdin: it sees EOF and exits.
            });
        }

        if let Some(pipe) = stderr_pipe {
            let tail = Arc::clone(&stderr_tail);
            thread::spawn(move || {
                use std::io::BufRead;
                let mut reader = std::io::BufReader::new(pipe);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => break,
                        Ok(_) => push_stderr(&tail, line.trim_end().to_string()),
                        Err(_) => break,
                    }
                }
            });
        }

        // stdout reader: NDJSON in, typed frames out (S6).
        if let Some(pipe) = stdout_pipe {
            let sink = Arc::clone(&self.sink);
            let key_owned = key.to_string();
            let pending = Arc::clone(&pending);
            let ready = Arc::clone(&ready);
            let stderr_tail = Arc::clone(&stderr_tail);
            let startup = startup_tx.clone();
            thread::spawn(move || {
                let mut pipe = pipe;
                let mut codec = FrameCodec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    let filled = match pipe.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(err) => {
                            push_stderr(&stderr_tail, format!("stdout read error: {err}"));
                            break;
                        }
                    };
                    for frame in codec.push_bytes(&chunk[..filled]) {
                        dispatch_frame(
                            frame,
                            &key_owned,
                            &sink,
                            &pending,
                            &ready,
                            &startup,
                            &stderr_tail,
                        );
                    }
                }
                // A last line without its newline still belongs to the stream.
                for frame in codec.finish() {
                    dispatch_frame(
                        frame,
                        &key_owned,
                        &sink,
                        &pending,
                        &ready,
                        &startup,
                        &stderr_tail,
                    );
                }
            });
        }

        // exit watcher: owns the `Child`, classifies the exit, settles waiters.
        {
            let sink = Arc::clone(&self.sink);
            let key_owned = key.to_string();
            let status = Arc::clone(&status);
            let stopping = Arc::clone(&stopping);
            let ready = Arc::clone(&ready);
            let stderr_tail = Arc::clone(&stderr_tail);
            let pending = Arc::clone(&pending);
            let exit = Arc::clone(&exit);
            thread::spawn(move || {
                let code = child.wait().ok().and_then(|status| status.code());
                // Nobody can answer a pending request once the child is gone.
                lock(&pending).clear();
                let deliberate = stopping.load(Ordering::SeqCst);
                let final_status = if deliberate {
                    RuntimeStatus::Stopped
                } else {
                    RuntimeStatus::Crashed
                };
                // A child that never became ready has no running session to
                // report an exit for: its startup owner decides the outcome.
                if deliberate || ready.load(Ordering::SeqCst) {
                    set_status(&sink, &key_owned, &status, final_status, None);
                    sink(
                        &key_owned,
                        &AgentEvent::ProcessExit {
                            code,
                            stderr_tail: tail_snapshot(&stderr_tail),
                        },
                    );
                } else {
                    // A child that never became ready has no running session to
                    // report an exit for: its startup owner decides the outcome,
                    // but the exit code still explains the failure.
                    push_stderr(
                        &stderr_tail,
                        format!(
                            "pi exited before ready (code {})",
                            code.map_or_else(|| "none".to_string(), |code| code.to_string())
                        ),
                    );
                }
                exit.mark(code);
                let _ = startup_tx.send(StartupSignal::Exited);
            });
        }

        let outcome = self.await_startup(&startup_rx, &stdin_tx);
        match outcome {
            StartupOutcome::Ready => {
                let session = Arc::new(SessionProcess {
                    pid,
                    stdin: stdin_tx,
                    codec: Mutex::new(FrameCodec::new()),
                    pending: Arc::clone(&pending),
                    status: Arc::clone(&status),
                    stopping,
                    stderr_tail: Arc::clone(&stderr_tail),
                    exit,
                });
                lock(&self.sessions).insert(key.to_string(), session);
                set_status(&self.sink, key, &status, RuntimeStatus::Ready, None);
                SpawnReport {
                    outcome,
                    pid: Some(pid),
                    status: RuntimeStatus::Ready,
                    stderr_tail: tail_snapshot(&stderr_tail),
                }
            }
            _ => {
                // Close stdin, then reap the whole tree: a timed-out or crashed
                // attempt must leave no process behind for the retry to inherit.
                let _ = stdin_tx.send(StdinMsg::Close);
                force_stop(pid, &exit, &self.timeouts);
                let tail = tail_snapshot(&stderr_tail);
                let error = Some(tail.join("\n")).filter(|text| !text.trim().is_empty());
                set_status(
                    &self.sink,
                    key,
                    &status,
                    RuntimeStatus::Crashed,
                    error,
                );
                SpawnReport {
                    outcome,
                    pid: Some(pid),
                    status: RuntimeStatus::Crashed,
                    stderr_tail: tail,
                }
            }
        }
    }

    /// Waits for the readiness probe's correlated answer until the deadline,
    /// resending the probe because early stdin writes can race pi's reader.
    fn await_startup(
        &self,
        startup: &mpsc::Receiver<StartupSignal>,
        stdin: &mpsc::Sender<StdinMsg>,
    ) -> StartupOutcome {
        let probe = format!(
            "{{\"type\":\"{STARTUP_PROBE_COMMAND}\",\"id\":\"{STARTUP_PROBE_ID}\"}}\n"
        );
        let tick = self
            .timeouts
            .probe_interval
            .max(Duration::from_millis(1));
        let deadline = Instant::now() + self.timeouts.ready;
        let mut next_probe = Instant::now();
        loop {
            let now = Instant::now();
            if now >= next_probe {
                if stdin.send(StdinMsg::Line(probe.clone())).is_err() {
                    // stdin is already gone: the child cannot answer any more.
                    return StartupOutcome::Crashed;
                }
                next_probe = now + tick;
            }
            if now >= deadline {
                return StartupOutcome::Timeout;
            }
            match startup.recv_timeout((deadline - now).min(tick)) {
                Ok(StartupSignal::Ready) => return StartupOutcome::Ready,
                Ok(StartupSignal::Exited) => return StartupOutcome::Crashed,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return StartupOutcome::Crashed,
            }
        }
    }

    /// Sends one request and waits for its correlated response.
    pub fn send(
        &self,
        key: &str,
        command: &str,
        fields: Map<String, Value>,
    ) -> Result<RpcResponse, ProcessError> {
        let session = lock(&self.sessions)
            .get(key)
            .cloned()
            .ok_or_else(|| ProcessError::NotRunning(key.to_string()))?;
        let encoded = lock(&session.codec).encode_request(command, fields);
        let (waiter, response) = mpsc::sync_channel::<RpcResponse>(1);
        lock(&session.pending).insert(encoded.id.clone(), waiter);
        // The first request is what turns a ready session into a working one.
        set_status(
            &self.sink,
            key,
            &session.status,
            RuntimeStatus::Running,
            None,
        );
        if let Err(err) = session.stdin.send(StdinMsg::Line(encoded.line)) {
            lock(&session.pending).remove(&encoded.id);
            return Err(ProcessError::Transport(err.to_string()));
        }
        match response.recv_timeout(self.timeouts.response) {
            Ok(response) => Ok(response),
            Err(_) => {
                lock(&session.pending).remove(&encoded.id);
                Err(ProcessError::ResponseTimeout(format!(
                    "{command} (id {})",
                    encoded.id
                )))
            }
        }
    }

    /// Stops the session's child: stdin EOF, then signal the whole process tree
    /// after the grace periods. Idempotent, like `stopAndWait` for a dead child.
    pub fn stop_and_wait(&self, key: &str) -> Result<(), ProcessError> {
        let Some(session) = lock(&self.sessions).remove(key) else {
            return Ok(());
        };
        // Set before closing stdin: the exit watcher reads it to tell a deliberate
        // stop (Stopped) from an unexpected exit (Crashed).
        session.stopping.store(true, Ordering::SeqCst);
        lock(&session.pending).clear();
        let _ = session.stdin.send(StdinMsg::Close);
        force_stop(session.pid, &session.exit, &self.timeouts);
        if !session.exit.wait(Duration::from_millis(0)) {
            // The child ignored even the force kill. It is no longer addressable
            // through the manager, so report the terminal state explicitly.
            set_status(
                &self.sink,
                key,
                &session.status,
                RuntimeStatus::Stopped,
                None,
            );
        }
        Ok(())
    }

    /// Status of a live session, `None` when nothing is running under `key`.
    pub fn status(&self, key: &str) -> Option<SessionSnapshot> {
        let session = lock(&self.sessions).get(key).cloned()?;
        let status = *lock(&session.status);
        let error = if status == RuntimeStatus::Crashed {
            Some(tail_snapshot(&session.stderr_tail).join("\n")).filter(|text| !text.is_empty())
        } else {
            None
        };
        Some(SessionSnapshot {
            status,
            pid: session.pid,
            error,
        })
    }

    /// Stops every session. Called when the app exits: the children die with
    /// the host (S1).
    pub fn stop_all(&self) {
        let keys: Vec<String> = lock(&self.sessions).keys().cloned().collect();
        for key in keys {
            let _ = self.stop_and_wait(&key);
        }
    }

    /// Ids of the sessions that currently have a registered child, sorted so
    /// the answer is stable across calls. Reads the registry only: this never
    /// spawns or touches a process (S5, `live_session_ids`).
    pub fn live_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = lock(&self.sessions).keys().cloned().collect();
        ids.sort();
        ids
    }

    #[cfg(test)]
    fn contains(&self, key: &str) -> bool {
        lock(&self.sessions).contains_key(key)
    }
}

impl Drop for ProcessManager {
    fn drop(&mut self) {
        // S1: no pi outlives the host that owns it. Handles share the registry
        // through an `Arc`, so only the last one owns the children: dropping a
        // clone (a command's worker thread handle) must never kill a live pi.
        if Arc::strong_count(&self.sessions) == 1 {
            self.stop_all();
        }
    }
}

/// One managed child process and the channels into its threads.
struct SessionProcess {
    pid: u32,
    stdin: mpsc::Sender<StdinMsg>,
    codec: Mutex<FrameCodec>,
    pending: Arc<Mutex<HashMap<String, mpsc::SyncSender<RpcResponse>>>>,
    status: Arc<Mutex<RuntimeStatus>>,
    stopping: Arc<AtomicBool>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    exit: Arc<ExitSignal>,
}

/// Arguments of the pi RPC invocation. Mirror: Stark `process_manager.rs`
/// (steps 3 wrongly pin provider/model for every project, so only the two flags
/// Nabla needs are kept) and `pi-rpc-manager.ts` `buildPiArgs`.
/// S7 adds the explicit provider/model pair: the model selector decides them per
/// spawn, so pi never falls back to whatever provider it would pick itself.
///
/// The Nabla identity rides along as an appended system prompt so every session
/// knows what it is and what it can (and cannot) do; the text is versioned in
/// `resources/nabla-system-prompt.md` and embedded at compile time.
fn spawn_args(session_file: &Path, provider: &str, model: &str) -> Vec<String> {
    vec![
        "--mode".to_string(),
        "rpc".to_string(),
        "--session".to_string(),
        session_file.to_string_lossy().to_string(),
        "--provider".to_string(),
        provider.to_string(),
        "--model".to_string(),
        model.to_string(),
        // Nabla is its own harness: it must not inherit the user's terminal
        // harness extensions (gentle-ai injects an "el Gentleman" identity via
        // appendSystemPrompt), so extension discovery is disabled and only the
        // extensions Nabla itself needs are loaded back explicitly.
        "--no-extensions".to_string(),
        "-e".to_string(),
        "builtin:mcp".to_string(),
        "--append-system-prompt".to_string(),
        NABLA_SYSTEM_PROMPT.to_string(),
    ]
}

/// The Nabla identity, embedded at build time from the versioned markdown file
/// so it never depends on a runtime path or a packaged resource.
const NABLA_SYSTEM_PROMPT: &str = include_str!("../../../resources/nabla-system-prompt.md");

/// Exact variable names the engine needs to reach its providers and to run
/// (interpreter lookups, `~`, node modules).
const FORWARDED_ENV_EXACT: &[&str] = &[
    "PATH",
    "HOME",
    // Windows equivalents of the two above; the child would not start without them.
    "USERPROFILE",
    "SYSTEMROOT",
    "NODE_PATH",
    "COLORTERM",
    // pi's agent directory. Without it the child falls back to ~/.pi/agent, which
    // may not be where the user's pi keeps mcp.json, auth.json or models.json —
    // the symptom is an MCP list that answers "no servers configured" while the
    // user's own terminal sees them. Forwarding it also makes sessions and CLI
    // commands read the same configuration the user's pi reads.
    "PI_CODING_AGENT_DIR",
    "OPENAI_API_KEY",
    "CLAUDE_CODE_USE_VERTEX",
    "CLOUD_ML_REGION",
    "GCLOUD_PROJECT",
];

/// Provider families forwarded by prefix, whichever model the workspace uses.
const FORWARDED_ENV_PREFIXES: &[&str] = &["ANTHROPIC_", "GOOGLE_"];

/// Environment allowlist forwarded to pi. The child never inherits the whole
/// parent environment: only what the engine needs to reach its providers.
/// Mirror: Stark `process_manager.rs` `forwarded_env_vars`.
/// Shared with `cli.rs` (S1, S2 of `pi-cli-bridge.md`): the CLI bridge forwards
/// the very same allowlist instead of duplicating it.
pub(crate) fn forwarded_env<I>(vars: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (String, String)>,
{
    vars.into_iter()
        .filter(|(name, _)| {
            FORWARDED_ENV_EXACT
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(name))
                || FORWARDED_ENV_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
        })
        .collect()
}

/// Builds the headless child: piped stdio, no terminal, allowlisted env, own
/// process group on Unix (S1, S5).
fn build_command(
    pi_binary: &str,
    session_file: &Path,
    provider: &str,
    model: &str,
    cwd: &Path,
) -> Command {
    let mut command = Command::new(pi_binary);
    command.args(spawn_args(session_file, provider, model));
    command.current_dir(cwd);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.env_clear();
    for (name, value) in forwarded_env(std::env::vars()) {
        command.env(name, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group (pgid == child pid) makes kill(-pid) reach pi
        // AND every descendant it spawned.
        // Mirror: `pi-rpc-manager.ts` `detached: !IS_WINDOWS`.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // S1: no console window, ever.
        // SAFETY: a documented creation flag, no memory or pointer effects.
        unsafe {
            command.creation_flags(CREATE_NO_WINDOW);
        }
    }
    command
}

/// Windows creation flag: run without a console window (S1).
/// Shared with `cli.rs` so a CLI call never flashes a console either.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Windows force-kill invocation for a whole process tree (S5). Built here,
/// unconditionally, so the shape is unit-tested on every platform; only executed
/// under `cfg(windows)`.
#[cfg_attr(not(windows), allow(dead_code))]
fn kill_tree_args(pid: u32, force: bool) -> Vec<String> {
    let mut args = vec!["/T".to_string()];
    if force {
        args.push("/F".to_string());
    }
    args.push("/PID".to_string());
    args.push(pid.to_string());
    args
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: i32) {
    // SAFETY: kill() only reads its two integer arguments. A pid that is already
    // gone (or a group already empty) returns ESRCH, which is the desired state.
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

// The terminate/kill pair is shared with `cli.rs`: one process-group kill path
// for every child Nabla owns, never a second, weaker one.
#[cfg(unix)]
pub(crate) fn signal_terminate(pid: u32) {
    signal_group(pid, libc::SIGTERM);
}

#[cfg(unix)]
pub(crate) fn signal_kill(pid: u32) {
    signal_group(pid, libc::SIGKILL);
}

#[cfg(windows)]
pub(crate) fn signal_terminate(pid: u32) {
    run_taskkill(pid, false);
}

#[cfg(windows)]
pub(crate) fn signal_kill(pid: u32) {
    run_taskkill(pid, true);
}

#[cfg(windows)]
fn run_taskkill(pid: u32, force: bool) {
    let _ = Command::new("taskkill")
        .args(kill_tree_args(pid, force))
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::AgentEvent;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;
    use std::thread;
    use std::time::Instant;

    /// Scratch directory outside the worktree, removed on drop.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let mut dir = std::env::temp_dir();
            dir.push(format!(
                "nabla_pi_{}_{}_{tag}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).expect("create scratch dir");
            Scratch { dir }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Events collected from the manager's sink.
    #[derive(Default)]
    struct Collected {
        events: StdMutex<Vec<(String, AgentEvent)>>,
    }

    impl Collected {
        fn sink(self: &Arc<Self>) -> EventSink {
            let collected = Arc::clone(self);
            Arc::new(move |key: &str, event: &AgentEvent| {
                collected
                    .events
                    .lock()
                    .unwrap()
                    .push((key.to_string(), event.clone()))
            })
        }

        fn snapshot(&self) -> Vec<(String, AgentEvent)> {
            self.events.lock().unwrap().clone()
        }

        /// Waits until `check` accepts the collected events, so the test never
        /// races the reader thread. Panics with what it saw on timeout.
        fn wait_for(
            &self,
            what: &str,
            check: impl Fn(&[(String, AgentEvent)]) -> bool,
        ) -> Vec<(String, AgentEvent)> {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let events = self.snapshot();
                if check(&events) {
                    return events;
                }
                assert!(Instant::now() < deadline, "timed out waiting for {what}: {events:#?}");
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    fn message_updates(events: &[(String, AgentEvent)]) -> Vec<(Option<String>, Option<String>)> {
        events
            .iter()
            .filter_map(|(_, event)| match event {
                AgentEvent::MessageUpdate {
                    session_id,
                    assistant_message_event,
                    ..
                } => Some((
                    session_id.clone(),
                    assistant_message_event.delta.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    fn statuses(events: &[(String, AgentEvent)]) -> Vec<RuntimeStatus> {
        events
            .iter()
            .filter_map(|(_, event)| match event {
                AgentEvent::ProcessStatus { status, .. } => Some(*status),
                _ => None,
            })
            .collect()
    }

    /// Provider/model used by lifecycle tests that do not care about S7: the
    /// spawn signature requires them, and any real pair works for a fake engine.
    const TEST_PROVIDER: &str = "nan";
    const TEST_MODEL: &str = "deepseek-v4-flash";

    fn test_timeouts() -> Timeouts {
        Timeouts {
            ready: Duration::from_millis(400),
            probe_interval: Duration::from_millis(50),
            stop_grace: Duration::from_millis(800),
            kill_grace: Duration::from_millis(400),
            response: Duration::from_secs(5),
        }
    }

    /// The happy-path fake engine: answers the readiness probe, then for every
    /// `prompt` frame emits two text deltas and a correlated response. No real pi,
    /// no network (S8). It first records its own argv, one argument per line, so
    /// a test can prove which flags reached the child.
    const FAKE_PI: &str = r#"#!/bin/sh
printf '%s\n' "$@" >> '@ARGS@'
printf 'attempt\n' >> '@ATTEMPTS@'
printf '%s\n' '{"type":"response","id":"__startup_probe__","command":"get_state","success":true,"data":{}}'
while IFS= read -r line; do
  case "$line" in
    *'"type":"prompt"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
      printf '%s\n' '{"type":"agent_start"}'
      printf '%s\n' '{"type":"message_update","message":{"role":"assistant"},"assistantMessageEvent":{"type":"text_delta","delta":"Hel","contentIndex":0}}'
      printf '%s\n' '{"type":"message_update","message":{"role":"assistant"},"assistantMessageEvent":{"type":"text_delta","delta":"lo","contentIndex":0}}'
      printf '{"type":"response","id":"%s","command":"prompt","success":true,"data":null}\n' "$id"
      ;;
  esac
done
printf 'exited\n' >> '@MARKER@'
"#;

    /// Crash-before-ready: records the attempt and exits non-zero at once.
    const FAKE_PI_CRASH: &str = r#"#!/bin/sh
printf 'attempt\n' >> '@ATTEMPTS@'
exit 3
"#;

    /// Silent child: never answers the probe, so startup can only time out.
    const FAKE_PI_SILENT: &str = r#"#!/bin/sh
printf 'attempt\n' >> '@ATTEMPTS@'
sleep 30
"#;

    #[cfg(unix)]
    fn write_script(scratch: &Scratch, name: &str, attempts: &Path, marker: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch.path(name);
        // Every fake script can log its argv here: the file sits next to the
        // script, outside the worktree, and only exists when the script writes it.
        let args_log = path.with_extension("args");
        let script = body
            .replace("@ATTEMPTS@", &attempts.display().to_string())
            .replace("@MARKER@", &marker.display().to_string())
            .replace("@ARGS@", &args_log.display().to_string());
        std::fs::write(&path, script).expect("write fake pi");
        let mut perms = std::fs::metadata(&path).expect("stat fake pi").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod fake pi");
        path
    }

    /// Arguments the fake engine recorded for `name`, waiting briefly because the
    /// writer is the child process, not the test thread.
    #[cfg(unix)]
    fn recorded_args(scratch: &Scratch, name: &str) -> Vec<String> {
        let args_log = scratch.path(name).with_extension("args");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(raw) = std::fs::read_to_string(&args_log)
                && !raw.trim().is_empty()
            {
                return raw.lines().map(|line| line.to_string()).collect();
            }
            assert!(
                Instant::now() < deadline,
                "fake pi never recorded its arguments at {}",
                args_log.display()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    fn attempts_count(path: &Path) -> usize {
        std::fs::read_to_string(path)
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    // S2/S3/S7: the invocation shape. Provider and model are passed per spawn
    // (S7), so pi can never fall back to the provider it would pick on its own.
    #[test]
    fn spawn_args_match_the_pi_rpc_invocation() {
        let args = spawn_args(
            Path::new("/tmp/sessions/1-1700000000.jsonl"),
            "nan",
            "deepseek-v4-flash",
        );
        assert_eq!(
            args,
            vec![
                "--mode".to_string(),
                "rpc".to_string(),
                "--session".to_string(),
                "/tmp/sessions/1-1700000000.jsonl".to_string(),
                "--provider".to_string(),
                "nan".to_string(),
                "--model".to_string(),
                "deepseek-v4-flash".to_string(),
                "--no-extensions".to_string(),
                "-e".to_string(),
                "builtin:mcp".to_string(),
                "--append-system-prompt".to_string(),
                NABLA_SYSTEM_PROMPT.to_string(),
            ]
        );
        // The identity actually says who we are: it is not an empty placeholder.
        assert!(NABLA_SYSTEM_PROMPT.contains("Nabla"));
        // And it must never claim the terminal harness's identity.
        assert!(!NABLA_SYSTEM_PROMPT.contains("el Gentleman"));
    }

    // S7: the configured provider and model reach the child process, in the
    // exact flag order pi expects. Asserted on the child's own argv, not on the
    // builder, so a broken pipe of the setting would show up here.
    #[cfg(unix)]
    #[test]
    fn spawn_forwards_the_configured_provider_and_model() {
        let scratch = Scratch::new("provider_args");
        let attempts = scratch.path("attempts");
        let marker = scratch.path("marker");
        let script = write_script(&scratch, "fake-pi.sh", &attempts, &marker, FAKE_PI);
        let collected = Arc::new(Collected::default());
        let manager = ProcessManager::with_timeouts(collected.sink(), test_timeouts());

        let report = manager
            .spawn(
                "s1",
                script.to_str().unwrap(),
                &scratch.path("s1.jsonl"),
                "nan",
                "glm5.3",
                &scratch.dir,
            )
            .expect("spawn");
        assert_eq!(report.outcome, StartupOutcome::Ready, "stderr: {:?}", report.stderr_tail);

        let args = recorded_args(&scratch, "fake-pi.sh");
        let provider_at = args.iter().position(|arg| arg == "--provider");
        let model_at = args.iter().position(|arg| arg == "--model");
        assert_eq!(
            provider_at.and_then(|at| args.get(at + 1)).map(String::as_str),
            Some("nan"),
            "the child must receive --provider nan, got {args:?}"
        );
        assert_eq!(
            model_at.and_then(|at| args.get(at + 1)).map(String::as_str),
            Some("glm5.3"),
            "the child must receive --model glm5.3, got {args:?}"
        );
        let session_at = args.iter().position(|arg| arg == "--session").expect("--session");
        assert!(
            session_at < provider_at.unwrap_or(0) && provider_at.unwrap() < model_at.unwrap(),
            "flags follow --session in provider-then-model order: {args:?}"
        );
        manager.stop_all();
    }

    // S1/S5: the child never inherits the host environment wholesale.
    #[test]
    fn env_allowlist_forwards_only_engine_variables() {
        let vars: Vec<(String, String)> = vec![
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/home/user"),
            ("NODE_PATH", "/node/modules"),
            ("PI_CODING_AGENT_DIR", "/home/user/.omp/agent"),
            ("PI_SESSION_FILE", "must-not-leak"),
            ("ANTHROPIC_API_KEY", "anthropic-token"),
            ("GOOGLE_CLOUD_PROJECT", "project"),
            ("OPENAI_API_KEY", "openai-token"),
            ("AWS_SECRET_ACCESS_KEY", "must-not-leak"),
            ("SECRET_TOKEN", "must-not-leak"),
            ("RANDOM_THING", "must-not-leak"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

        let kept = forwarded_env(vars);
        let names: Vec<&str> = kept.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"PATH"));
        assert!(names.contains(&"HOME"));
        assert!(names.contains(&"NODE_PATH"));
        // The user's agent dir must travel with the child, or it reads another
        // pi's configuration (the MCP list came back empty without it).
        assert!(names.contains(&"PI_CODING_AGENT_DIR"));
        // But never our own session identity: the child must not inherit it.
        assert!(!names.contains(&"PI_SESSION_FILE"));
        assert!(names.contains(&"ANTHROPIC_API_KEY"));
        assert!(names.contains(&"GOOGLE_CLOUD_PROJECT"));
        assert!(names.contains(&"OPENAI_API_KEY"));
        assert!(
            !names.iter().any(|n| n.starts_with("AWS_") || n.contains("SECRET") || n.starts_with("RANDOM_")),
            "allowlist leaked a non-engine variable: {names:?}"
        );
        // Values are forwarded untouched.
        let path = kept.iter().find(|(k, _)| k == "PATH").expect("PATH");
        assert_eq!(path.1, "/usr/bin:/bin");
    }

    // S5: the Windows kill targets the whole tree, not just the child (the
    // defect the Stark reference has, and the opposite of libc's Unix-only kill).
    #[test]
    fn windows_kill_command_targets_the_process_tree() {
        assert_eq!(
            kill_tree_args(4242, false),
            vec!["/T".to_string(), "/PID".to_string(), "4242".to_string()]
        );
        assert_eq!(
            kill_tree_args(4242, true),
            vec![
                "/T".to_string(),
                "/F".to_string(),
                "/PID".to_string(),
                "4242".to_string()
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn spawn_reaches_ready_then_forwards_streamed_events() {
        let scratch = Scratch::new("ready");
        let attempts = scratch.path("attempts");
        let marker = scratch.path("marker");
        let script = write_script(&scratch, "fake-pi.sh", &attempts, &marker, FAKE_PI);
        let collected = Arc::new(Collected::default());
        let manager = ProcessManager::with_timeouts(collected.sink(), test_timeouts());

        let report = manager
            .spawn(
            "s1",
            script.to_str().unwrap(),
            &scratch.path("s1.jsonl"),
            TEST_PROVIDER,
            TEST_MODEL,
            &scratch.dir,
        )
            .expect("spawn");
        assert_eq!(report.outcome, StartupOutcome::Ready, "stderr: {:?}", report.stderr_tail);
        assert_eq!(report.status, RuntimeStatus::Ready);
        assert!(report.pid.unwrap_or(0) > 0);
        assert_eq!(attempts_count(&attempts), 1, "identical process must not respawn");

        // Readiness is a state transition the webview can observe (S4).
        collected.wait_for("ready status", |events| {
            statuses(events).contains(&RuntimeStatus::Ready)
        });
        assert_eq!(manager.status("s1").map(|s| s.status), Some(RuntimeStatus::Ready));

        // S6: pi's streamed assistant text is decoded and forwarded per session.
        let response = manager
            .send(
                "s1",
                "prompt",
                {
                    let mut map = Map::new();
                    map.insert("message".to_string(), json!("hi"));
                    map
                },
            )
            .expect("prompt response");
        assert_eq!(response.id.as_deref(), Some("1"), "response must echo our request id");
        assert_eq!(response.command.as_deref(), Some("prompt"));
        assert!(response.success);
        assert_eq!(manager.status("s1").map(|s| s.status), Some(RuntimeStatus::Running));

        let events = collected.wait_for("streamed deltas", |events| {
            message_updates(events).len() >= 2
        });
        assert_eq!(
            message_updates(&events),
            vec![
                (Some("s1".to_string()), Some("Hel".to_string())),
                (Some("s1".to_string()), Some("lo".to_string())),
            ],
            "deltas arrive in order and are stamped with their session"
        );
        let ordering = statuses(&events);
        assert_eq!(
            ordering.first(),
            Some(&RuntimeStatus::Spawning),
            "spawn announces Spawning before Ready: {ordering:?}"
        );
        assert!(ordering.contains(&RuntimeStatus::Running), "{ordering:?}");
        assert!(
            events.iter().any(|(key, event)| key == "s1"
                && matches!(event, AgentEvent::AgentStart)),
            "agent_start forwarded"
        );

        // S4: a graceful stop terminates the child and reports Stopped.
        manager.stop_and_wait("s1").expect("stop");
        assert!(
            !manager.contains("s1"),
            "a stopped session must not stay registered"
        );
        assert_eq!(manager.status("s1"), None);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let log = std::fs::read_to_string(&marker).unwrap_or_default();
        assert!(log.contains("exited"), "fake pi never reached its exit path: {log:?}");
        let events = collected.wait_for("stopped status", |events| {
            statuses(events).contains(&RuntimeStatus::Stopped)
        });
        assert!(
            events.iter().any(|(key, event)| key == "s1"
                && matches!(event, AgentEvent::ProcessExit { code: Some(0), .. })),
            "synthetic process_exit with a clean code: {events:#?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn crash_before_ready_is_retried_exactly_once() {
        let scratch = Scratch::new("crash");
        let attempts = scratch.path("attempts");
        let marker = scratch.path("marker");
        let script = write_script(&scratch, "fake-pi.sh", &attempts, &marker, FAKE_PI_CRASH);
        let collected = Arc::new(Collected::default());
        let manager = ProcessManager::with_timeouts(collected.sink(), test_timeouts());

        let report = manager
            .spawn(
            "s1",
            script.to_str().unwrap(),
            &scratch.path("s1.jsonl"),
            TEST_PROVIDER,
            TEST_MODEL,
            &scratch.dir,
        )
            .expect("spawn attempt");
        assert_eq!(report.outcome, StartupOutcome::Crashed);
        assert_eq!(report.status, RuntimeStatus::Crashed);
        assert_eq!(
            attempts_count(&attempts),
            STARTUP_MAX_ATTEMPTS,
            "a crash before ready is retried exactly once"
        );
        assert!(!manager.contains("s1"), "a crashed startup leaves no session");

        let events = collected.wait_for("crashed status", |events| {
            statuses(events).contains(&RuntimeStatus::Crashed)
        });
        assert!(
            statuses(&events).contains(&RuntimeStatus::Spawning),
            "the leading Spawning transition is reported: {events:#?}"
        );
        // The startup failure is not a post-ready exit, so no process_exit is
        // invented for it.
        assert!(
            !events
                .iter()
                .any(|(_, event)| matches!(event, AgentEvent::ProcessExit { .. })),
            "startup failure must not fake a running session's exit: {events:#?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn startup_timeout_is_never_retried() {
        let scratch = Scratch::new("timeout");
        let attempts = scratch.path("attempts");
        let marker = scratch.path("marker");
        let script = write_script(&scratch, "fake-pi.sh", &attempts, &marker, FAKE_PI_SILENT);
        let collected = Arc::new(Collected::default());
        let manager = ProcessManager::with_timeouts(collected.sink(), test_timeouts());

        let started = Instant::now();
        let report = manager
            .spawn(
            "s1",
            script.to_str().unwrap(),
            &scratch.path("s1.jsonl"),
            TEST_PROVIDER,
            TEST_MODEL,
            &scratch.dir,
        )
            .expect("spawn attempt");
        assert_eq!(report.outcome, StartupOutcome::Timeout);
        assert_eq!(report.status, RuntimeStatus::Crashed);
        assert_eq!(
            attempts_count(&attempts),
            1,
            "a timeout is never retried: respawning would burn another full deadline"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the injected deadline must bound the wait, took {:?}",
            started.elapsed()
        );
        assert_eq!(manager.status("s1"), None);
    }

    #[cfg(unix)]
    #[test]
    fn spawn_is_one_process_per_session() {
        let scratch = Scratch::new("duplicate");
        let attempts = scratch.path("attempts");
        let marker = scratch.path("marker");
        let script = write_script(&scratch, "fake-pi.sh", &attempts, &marker, FAKE_PI);
        let collected = Arc::new(Collected::default());
        let manager = ProcessManager::with_timeouts(collected.sink(), test_timeouts());

        manager
            .spawn(
            "s1",
            script.to_str().unwrap(),
            &scratch.path("s1.jsonl"),
            TEST_PROVIDER,
            TEST_MODEL,
            &scratch.dir,
        )
            .expect("first spawn");
        let second = manager.spawn(
            "s1",
            script.to_str().unwrap(),
            &scratch.path("s1.jsonl"),
            TEST_PROVIDER,
            TEST_MODEL,
            &scratch.dir,
        );
        assert!(
            matches!(second, Err(ProcessError::AlreadyRunning(_))),
            "S3: one process per session, got {second:?}"
        );
        // A second session gets its own process.
        manager
            .spawn(
            "s2",
            script.to_str().unwrap(),
            &scratch.path("s2.jsonl"),
            TEST_PROVIDER,
            TEST_MODEL,
            &scratch.dir,
        )
            .expect("second session spawn");
        assert!(manager.contains("s1") && manager.contains("s2"));
        assert_eq!(attempts_count(&attempts), 2);

        manager.stop_all();
        assert!(!manager.contains("s1") && !manager.contains("s2"));
    }

    // Handles are cheap to clone so a Tauri command can move one onto a worker
    // thread instead of blocking the IPC thread on a spawn. Every handle shares
    // the one registry, and only the last one owns the children (S1): dropping a
    // clone must never kill a live pi.
    #[cfg(unix)]
    #[test]
    fn cloning_the_manager_shares_the_registry_and_never_stops_the_children() {
        let scratch = Scratch::new("clone");
        let attempts = scratch.path("attempts");
        let marker = scratch.path("marker");
        let script = write_script(&scratch, "fake-pi.sh", &attempts, &marker, FAKE_PI);
        let collected = Arc::new(Collected::default());
        let manager = ProcessManager::with_timeouts(collected.sink(), test_timeouts());

        let report = manager
            .spawn(
                "s1",
                script.to_str().unwrap(),
                &scratch.path("s1.jsonl"),
                TEST_PROVIDER,
                TEST_MODEL,
                &scratch.dir,
            )
            .expect("spawn");
        assert_eq!(report.outcome, StartupOutcome::Ready, "stderr: {:?}", report.stderr_tail);
        let pid = report.pid.expect("a ready child has a pid");

        let worker = manager.clone();
        assert_eq!(
            worker.live_ids(),
            vec!["s1".to_string()],
            "a clone addresses the same registry, not a second one"
        );
        assert_eq!(
            worker.status("s1"),
            manager.status("s1"),
            "both handles report the same live child"
        );

        drop(worker);
        assert_eq!(
            manager.status("s1").map(|snapshot| snapshot.pid),
            Some(pid),
            "dropping a clone must not stop the child it did not start"
        );
        assert_eq!(
            manager.status("s1").map(|snapshot| snapshot.status),
            Some(RuntimeStatus::Ready),
            "the registry entry survives the clone's drop"
        );

        manager.stop_and_wait("s1").expect("stop");
        assert!(manager.live_ids().is_empty());
    }

    // S5: the live-id list is read from the registry, so it reflects a spawned
    // session without spawning anything and drops the id once the child is
    // stopped. The ids must never be empty for a live child (the list paints
    // the per-session state in the UI).
    #[cfg(unix)]
    #[test]
    fn live_ids_lists_a_spawned_session_and_drops_it_after_stop() {
        let scratch = Scratch::new("live_ids");
        let attempts = scratch.path("attempts");
        let marker = scratch.path("marker");
        let script = write_script(&scratch, "fake-pi.sh", &attempts, &marker, FAKE_PI);
        let collected = Arc::new(Collected::default());
        let manager = ProcessManager::with_timeouts(collected.sink(), test_timeouts());
        assert_eq!(manager.live_ids(), Vec::<String>::new(), "nothing spawned yet");

        manager
            .spawn(
                "s1",
                script.to_str().unwrap(),
                &scratch.path("s1.jsonl"),
                TEST_PROVIDER,
                TEST_MODEL,
                &scratch.dir,
            )
            .expect("first spawn");
        manager
            .spawn(
                "s2",
                script.to_str().unwrap(),
                &scratch.path("s2.jsonl"),
                TEST_PROVIDER,
                TEST_MODEL,
                &scratch.dir,
            )
            .expect("second spawn");
        assert_eq!(
            manager.live_ids(),
            vec!["s1".to_string(), "s2".to_string()],
            "live ids come from the registry and are stable"
        );
        assert_eq!(attempts_count(&attempts), 2, "reading live ids never spawns");

        manager.stop_and_wait("s1").expect("stop s1");
        assert_eq!(manager.live_ids(), vec!["s2".to_string()]);
        manager.stop_all();
        assert!(manager.live_ids().is_empty(), "a stopped session is not live");
    }
}
