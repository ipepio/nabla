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
// - The child is loaded with Nabla's own extensions (`nabla-ask`, the permission
//   gate) and never inherits the terminal harness's. The permission gate reads
//   the current mode from `<data_dir>/permissions.json` through the
//   `NABLA_DATA_DIR` variable: env is fixed at spawn time, so the MODE is never
//   in it (L4 of `permission-modes.md`).
//
// Wire references (read-only clones):
// - /tmp/nabla-pi-desktops.vYixxw/FaqFirebase/src/main/pi-rpc-manager.ts
//   (lifecycle, probe, retry rule, stop/kill sequencing)
// - /tmp/nabla-pi-desktops.vYixxw/StarkInternationalAI/src-tauri/src/process_manager.rs
//   (spawn args, env allowlist, per-session map, synthetic process_exit)

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::rpc::{
    AgentEvent, Frame, FrameCodec, RpcResponse, RuntimeStatus, StopReason, UiAnswer,
    STARTUP_PROBE_COMMAND, STARTUP_PROBE_ID, encode_ui_response,
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
    /// Grace for a stopped turn to settle after `abort` (S2 of
    /// `turn-continuity.md`) before the child is killed. Two seconds: pi closes
    /// an aborted turn in-process (it marks the run aborted, unwinds its loop
    /// and answers the request), so nothing here waits on a model or on a tool
    /// to finish. It is long enough for a child that is flushing its frames
    /// through a busy pipe and short enough that a stuck one cannot hold the
    /// stop, whose kill fallback already takes up to `stop_grace + kill_grace`
    /// (2.5 s) on top of it.
    pub abort_grace: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            ready: Duration::from_secs(120),
            probe_interval: Duration::from_millis(750),
            stop_grace: Duration::from_millis(1500),
            kill_grace: Duration::from_millis(1000),
            response: Duration::from_secs(60),
            abort_grace: Duration::from_secs(2),
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

/// What one graceful stop ended up doing (S2 of `turn-continuity.md`), so the
/// caller can tell a session that survived from one that had to be killed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortOutcome {
    /// pi closed the turn on its own: nothing was killed and the session is
    /// still alive and addressable.
    Settled,
    /// pi did not settle within `abort_grace`: the child was stopped exactly as
    /// a stop always did before this feature.
    Killed,
    /// Nothing was running under the key: nothing to abort and nothing to kill.
    NotRunning,
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

/// Settlement latch of one session's current run (S2 of `turn-continuity.md`).
///
/// A stop asks pi to `abort` and then waits here instead of killing the child.
/// pi is what fires it, and it fires on any of the three frames that mean the
/// run this stop asked about is over:
///
/// - `turn_end` with `stopReason: "aborted"` — the turn closed on pi's terms,
///   carrying the reason the UI announces;
/// - a successful `abort` response — pi answers that request only once the
///   session is idle (`docs/rpc-commands.md`), which also covers a stop with no
///   turn in flight;
/// - the child's exit — a dead child has nothing left to settle.
///
/// It is reset before every stop, so only the settlement of *this* stop can
/// release it.
#[derive(Default)]
struct TurnSettled {
    fired: Mutex<bool>,
    settled: Condvar,
}

impl TurnSettled {
    fn fire(&self) {
        *lock(&self.fired) = true;
        self.settled.notify_all();
    }

    fn reset(&self) {
        *lock(&self.fired) = false;
    }

    /// True when it fired within `timeout`.
    fn wait(&self, timeout: Duration) -> bool {
        let guard = lock(&self.fired);
        let (guard, _) = self
            .settled
            .wait_timeout_while(guard, timeout, |fired| !*fired)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard
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

/// One decoded frame, delivered to the handles that answer for its session.
/// Bundled so the decode path takes one argument instead of eight (a new handle
/// is added in one place, not at every call site).
struct FrameRouting<'a> {
    /// Registry key of the session these frames belong to.
    key: &'a str,
    /// The only bridge from a pi process to the webview (S6).
    sink: &'a EventSink,
    /// Correlated waiters of the requests still in flight (R2).
    pending: &'a Mutex<HashMap<String, mpsc::SyncSender<RpcResponse>>>,
    /// The startup probe's answer latched: the child is live.
    ready: &'a AtomicBool,
    /// Startup outcome of this child, for `await_startup`.
    startup: &'a mpsc::Sender<StartupSignal>,
    /// Bounded stderr tail, reported when the child dies (S4).
    stderr_tail: &'a Mutex<VecDeque<String>>,
    /// Latch a stop waits on instead of killing (S2 of `turn-continuity.md`).
    settled: &'a TurnSettled,
}

/// Routes one decoded frame: responses to their waiting request (or the startup
/// probe), events to the sink, undecodable lines to the stderr tail — and it is
/// also where a stop learns that pi closed the turn it asked to abort (S2 of
/// `turn-continuity.md`).
fn dispatch_frame(frame: Frame, routing: &FrameRouting<'_>) {
    let FrameRouting {
        key,
        sink,
        pending,
        ready,
        startup,
        stderr_tail,
        settled,
    } = *routing;
    match frame {
        Frame::Response(response) => {
            if response.id.as_deref() == Some(STARTUP_PROBE_ID) {
                // Success or an "unknown command" error both prove the loop is
                // live: key off the correlated answer, not off stdout activity.
                ready.store(true, Ordering::SeqCst);
                let _ = startup.send(StartupSignal::Ready);
                return;
            }
            // A successful `abort` answer means pi is idle again (S2 of
            // `turn-continuity.md`): it is the settlement signal of a stop whose
            // turn had no `turn_end` of its own. Only success settles anything —
            // a refused abort leaves the run going, so the stop must fall through
            // to the kill instead of claiming a clean close.
            if response.success && response.command.as_deref() == Some("abort") {
                settled.fire();
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
        Frame::Event(event) => {
            // A turn pi closed because it was aborted is the settlement a stop
            // waits for; it is emitted before `agent_end` and `agent_settled`.
            if matches!(
                event,
                AgentEvent::TurnEnd {
                    stop_reason: Some(StopReason::Aborted),
                    ..
                }
            ) {
                settled.fire();
            }
            sink(key, &stamp_session(event, key))
        }
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
    /// The first-party extension that lets the agent ask the human a question
    /// (`agent-question-bridge.md`, S2). `None` when it could not be
    /// materialized: sessions still work, they just cannot ask anything.
    ask_extension: Option<PathBuf>,
    /// The permission gate (`permission-modes.md`, M2). `None` when it could not
    /// be materialized: sessions start the same, and their tools run UNGATED —
    /// that is the failure mode this field makes visible to the caller (see
    /// `lib.rs`, which reports it once at boot).
    permissions_extension: Option<PathBuf>,
    /// The absolute app data dir the gate reads its projection from (L4). `None`
    /// means the child never receives `NABLA_DATA_DIR`, so the gate cannot read
    /// any mode and falls back to `write` on every call — never to `yolo`.
    data_dir: Option<PathBuf>,
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
            ask_extension: None,
            permissions_extension: None,
            data_dir: None,
        }
    }

    /// Loads the question extension into every child this manager spawns. The
    /// path is the one `materialize_ask_extension` answered at boot; without it
    /// the agent has no way to ask the human anything (S1, S2).
    pub fn with_ask_extension(mut self, path: PathBuf) -> ProcessManager {
        self.ask_extension = Some(path);
        self
    }

    /// Loads the permission gate into every child this manager spawns (`-e`),
    /// next to the question extension. Without it every tool call runs ungated
    /// (`permission-modes.md`, M2).
    pub fn with_permissions_extension(mut self, path: PathBuf) -> ProcessManager {
        self.permissions_extension = Some(path);
        self
    }

    /// Tells every child which absolute data dir holds `permissions.json`, so
    /// the gate can read the mode on each tool call (L4). The MODE is never
    /// forwarded: a spawn-time environment could not change without restarting
    /// the session, which is exactly what S2 forbids.
    pub fn with_data_dir(mut self, path: PathBuf) -> ProcessManager {
        self.data_dir = Some(path);
        self
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
        // S2 of `turn-continuity.md`: the latch a stop waits on. It belongs to the
        // child, so a stop that settles never has to kill it.
        let settled = Arc::new(TurnSettled::default());
        let pending: Arc<Mutex<HashMap<String, mpsc::SyncSender<RpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let mut child = match build_command(
            pi_binary,
            session_file,
            provider,
            model,
            cwd,
            SpawnLayout {
                ask_extension: self.ask_extension.as_deref(),
                permissions_extension: self.permissions_extension.as_deref(),
                data_dir: self.data_dir.as_deref(),
            },
        )
        .spawn() {
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
            let settled = Arc::clone(&settled);
            thread::spawn(move || {
                let mut pipe = pipe;
                let mut codec = FrameCodec::new();
                let mut chunk = [0u8; 8192];
                let routing = FrameRouting {
                    key: &key_owned,
                    sink: &sink,
                    pending: &pending,
                    ready: &ready,
                    startup: &startup,
                    stderr_tail: &stderr_tail,
                    settled: &settled,
                };
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
                        dispatch_frame(frame, &routing);
                    }
                }
                // A last line without its newline still belongs to the stream.
                for frame in codec.finish() {
                    dispatch_frame(frame, &routing);
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
            let settled = Arc::clone(&settled);
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
                // A dead child settles nothing any more: releasing a stop that is
                // waiting on it here is what keeps a crash from costing the whole
                // abort grace.
                settled.fire();
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
                    settled,
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

    /// Answers one pending extension question (S3): the exact
    /// `extension_ui_response` line goes to the child's stdin, so the agent's
    /// blocked tool returns the human's answer and the turn continues.
    ///
    /// Deliberately not `send`: that allocates one of our request ids and waits
    /// for a correlated response. pi allocates the question's id, sends no
    /// answer to an answer, and a minted id here would be a protocol lie.
    pub fn answer_ui_request(
        &self,
        key: &str,
        request_id: &str,
        answer: &UiAnswer,
    ) -> Result<(), ProcessError> {
        let session = lock(&self.sessions)
            .get(key)
            .cloned()
            .ok_or_else(|| ProcessError::NotRunning(key.to_string()))?;
        let line = encode_ui_response(request_id, answer);
        session
            .stdin
            .send(StdinMsg::Line(line))
            .map_err(|err| ProcessError::Transport(err.to_string()))
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

    /// S2 of `turn-continuity.md`: stops the current turn through pi's own
    /// `abort`, so the turn lands in the transcript with its own stop reason
    /// instead of dying with the process.
    ///
    /// Order, and every step matters:
    ///
    /// 1. Reset the settlement latch, so only this stop can release it.
    /// 2. Write the `abort` frame — one correlated request, like every other
    ///    command; pi answers it only once the session is idle
    ///    (`docs/rpc-commands.md`), which is the settlement signal of a stop with
    ///    no turn in flight.
    /// 3. Wait up to `abort_grace` (2 s) for the latch: `turn_end` with
    ///    `stopReason: "aborted"`, the successful `abort` answer, or the child's
    ///    exit. Two seconds because pi closes an aborted turn in-process — no
    ///    model call, no tool wait — so the grace only covers flushing its
    ///    frames; the kill fallback below them costs up to 2.5 s more, and a stop
    ///    that never returned would be worse than a kill.
    /// 4. On settlement the child is *not* touched: it keeps its transcript, its
    ///    registry entry and its stdin, and reads `Ready` again. That is the
    ///    point of S2 — a stop is a pause, not the end of the session.
    /// 5. On timeout, or on a child that died while waiting, fall through to
    ///    `stop_and_wait`: today's kill, unchanged.
    ///
    /// Idempotent like `stop_and_wait`: a key with no child is nothing to abort
    /// and nothing to kill.
    pub fn abort_and_wait(&self, key: &str) -> Result<AbortOutcome, ProcessError> {
        let Some(session) = lock(&self.sessions).get(key).cloned() else {
            return Ok(AbortOutcome::NotRunning);
        };
        session.settled.reset();
        let encoded = lock(&session.codec).encode_request("abort", Map::new());
        if session.stdin.send(StdinMsg::Line(encoded.line)).is_err() {
            // stdin is gone: the child cannot settle anything any more, so the
            // only honest answer is the kill path (which also unregisters it).
            self.stop_and_wait(key)?;
            return Ok(AbortOutcome::Killed);
        }
        if !session.settled.wait(self.timeouts.abort_grace)
            || session.exit.wait(Duration::from_millis(0))
        {
            // No settlement in time, or the child left while we waited: kill it
            // exactly as a stop always did.
            self.stop_and_wait(key)?;
            return Ok(AbortOutcome::Killed);
        }
        // The turn closed on pi's terms: the session stays alive and idle.
        set_status(&self.sink, key, &session.status, RuntimeStatus::Ready, None);
        Ok(AbortOutcome::Settled)
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
    /// Settlement latch of the current run: what `abort_and_wait` waits on
    /// instead of killing the child (S2 of `turn-continuity.md`).
    settled: Arc<TurnSettled>,
}

/// What a child needs to find Nabla's own extensions and the data dir the
/// permission gate reads its projection from (L4 of `permission-modes.md`).
/// Bundled in one value so `spawn_args`/`build_command` stay readable instead of
/// carrying three more positional `Option`s each.
#[derive(Debug, Clone, Copy, Default)]
struct SpawnLayout<'a> {
    ask_extension: Option<&'a Path>,
    permissions_extension: Option<&'a Path>,
    /// Absolute `<data_dir>`; the MODE itself is never part of it.
    data_dir: Option<&'a Path>,
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
///
/// `layout.ask_extension` is the materialized `nabla-ask.ts` (see
/// `materialize_ask_extension`) and `layout.permissions_extension` the
/// materialized `nabla-permissions.ts` (`materialize_permissions_extension`):
/// `--no-extensions` turns off discovery, and `-e` loads exactly the extensions
/// Nabla needs back in — the built-in MCP support and these two.
fn spawn_args(
    session_file: &Path,
    provider: &str,
    model: &str,
    layout: SpawnLayout<'_>,
) -> Vec<String> {
    let mut args = vec![
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
    ];
    if let Some(path) = layout.ask_extension {
        // S1/S2 of `agent-question-bridge.md`: the three ask tools exist only
        // because this extension is loaded (`-e` survives `--no-extensions`).
        args.push("-e".to_string());
        args.push(path.to_string_lossy().to_string());
    }
    if let Some(path) = layout.permissions_extension {
        // M2 of `permission-modes.md`: the gate. Without this flag the agent's
        // tools run ungated, which is the one failure this feature must make
        // loud instead of silent.
        args.push("-e".to_string());
        args.push(path.to_string_lossy().to_string());
    }
    args.push("--append-system-prompt".to_string());
    args.push(NABLA_SYSTEM_PROMPT.to_string());
    args
}

/// The Nabla identity, embedded at build time from the versioned markdown file
/// so it never depends on a runtime path or a packaged resource.
const NABLA_SYSTEM_PROMPT: &str = include_str!("../../../resources/nabla-system-prompt.md");

/// The first-party pi extension that gives the agent its three ask tools (S1,
/// S2), embedded at build time like the identity above.
const ASK_EXTENSION_SOURCE: &str = include_str!("../../../resources/nabla-ask.ts");

/// The first-party pi extension that gates every tool call by the human's
/// permission mode (`permission-modes.md`, M2), embedded the same way.
const PERMISSIONS_EXTENSION_SOURCE: &str =
    include_str!("../../../resources/nabla-permissions.ts");

/// Writes the embedded extension into `<data_dir>/extensions/nabla-ask.ts` and
/// answers its absolute path, which is what `-e` receives at spawn.
///
/// Idempotent by content: the file is only rewritten when it differs from the
/// source embedded in this binary, so an app update replaces a stale copy and a
/// plain restart touches nothing. The returned path always exists when this
/// succeeds.
pub fn materialize_ask_extension(data_dir: &Path) -> std::io::Result<PathBuf> {
    materialize_extension(data_dir, "nabla-ask.ts", ASK_EXTENSION_SOURCE)
}

/// Writes the permission gate into `<data_dir>/extensions/nabla-permissions.ts`
/// and answers its absolute path, with the same content-idempotent rule as the
/// question extension above. On failure the session's tools are UNGATED (there
/// is no gate to load), so the caller must report it rather than ignore it.
pub fn materialize_permissions_extension(data_dir: &Path) -> std::io::Result<PathBuf> {
    materialize_extension(
        data_dir,
        "nabla-permissions.ts",
        PERMISSIONS_EXTENSION_SOURCE,
    )
}

/// The one materializer both extensions use: `extensions/<name>` under the data
/// dir, rewritten only when the embedded bytes differ.
fn materialize_extension(
    data_dir: &Path,
    name: &str,
    source: &str,
) -> std::io::Result<PathBuf> {
    let dir = data_dir.join("extensions");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    let current = std::fs::read_to_string(&path).ok();
    if current.as_deref() != Some(source) {
        std::fs::write(&path, source)?;
    }
    Ok(path)
}

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
    layout: SpawnLayout<'_>,
) -> Command {
    let mut command = Command::new(pi_binary);
    command.args(spawn_args(session_file, provider, model, layout));
    command.current_dir(cwd);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.env_clear();
    for (name, value) in forwarded_env(std::env::vars()) {
        command.env(name, value);
    }
    if let Some(dir) = layout.data_dir {
        // L4 of `permission-modes.md`: the gate reads `<data_dir>/permissions.json`
        // on every tool call, so a mode change affects the very next call of a
        // live session. Only the DIRECTORY travels: the mode itself never does,
        // because the environment is fixed at spawn time.
        command.env("NABLA_DATA_DIR", dir);
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
    use crate::rpc::{AgentEvent, StopReason};
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
            abort_grace: Duration::from_millis(600),
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

    /// A child that asks the human one question and records every answer line it
    /// receives on stdin, so the response path is asserted on the bytes the child
    /// actually read, not on the builder.
    const FAKE_PI_UI: &str = r#"#!/bin/sh
printf '%s\n' "$@" >> '@ARGS@'
printf 'attempt\n' >> '@ATTEMPTS@'
printf '%s\n' '{"type":"response","id":"__startup_probe__","command":"get_state","success":true,"data":{}}'
printf '%s\n' '{"type":"extension_ui_request","id":"ui-1","method":"select","title":"Which one?","options":["A","B"]}'
while IFS= read -r line; do
  case "$line" in
    *'"type":"extension_ui_response"'*) printf '%s\n' "$line" >> '@ANSWERS@' ;;
  esac
done
printf 'exited\n' >> '@MARKER@'
"#;

    /// A child that honours `abort` the way pi documents it: it closes the turn
    /// with `stopReason:"aborted"`, ends the run, settles, answers the request —
    /// and keeps reading stdin, which is the whole point of S2 (the session
    /// survives the stop).
    const FAKE_PI_ABORT: &str = r#"#!/bin/sh
printf '%s\n' "$@" >> '@ARGS@'
printf 'attempt\n' >> '@ATTEMPTS@'
printf '%s\n' '{"type":"response","id":"__startup_probe__","command":"get_state","success":true,"data":{}}'
while IFS= read -r line; do
  case "$line" in
    *'"type":"prompt"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
      printf '%s\n' '{"type":"agent_start"}'
      printf '%s\n' '{"type":"turn_start"}'
      printf '%s\n' '{"type":"message_update","message":{"role":"assistant"},"assistantMessageEvent":{"type":"text_delta","delta":"working","contentIndex":0}}'
      printf '{"type":"response","id":"%s","command":"prompt","success":true,"data":null}\n' "$id"
      ;;
    *'"type":"abort"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
      printf '%s\n' "$line" >> '@ABORTED@'
      printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","stopReason":"aborted"},"toolResults":[]}'
      printf '%s\n' '{"type":"agent_end","messages":[],"willRetry":false}'
      printf '%s\n' '{"type":"agent_settled","aborted":true}'
      printf '{"type":"response","id":"%s","command":"abort","success":true}\n' "$id"
      ;;
  esac
done
printf 'exited\n' >> '@MARKER@'
"#;

    /// A child that ignores `abort` completely: it records that the request
    /// arrived and emits nothing else, so only the bounded wait and the kill can
    /// end the stop.
    const FAKE_PI_ABORT_DEAF: &str = r#"#!/bin/sh
printf 'attempt\n' >> '@ATTEMPTS@'
printf '%s\n' '{"type":"response","id":"__startup_probe__","command":"get_state","success":true,"data":{}}'
while IFS= read -r line; do
  case "$line" in
    *'"type":"abort"'*) printf '%s\n' "$line" >> '@ABORTED@' ;;
  esac
done
printf 'exited\n' >> '@MARKER@'
"#;

    #[cfg(unix)]
    fn write_script(scratch: &Scratch, name: &str, attempts: &Path, marker: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch.path(name);
        // Every fake script can log its argv here: the file sits next to the
        // script, outside the worktree, and only exists when the script writes it.
        let args_log = path.with_extension("args");
        // And every script that answers a question logs the lines it read here.
        let answers_log = path.with_extension("answers");
        // Every script that receives `abort` logs the request line here.
        let aborted_log = path.with_extension("aborted");
        let script = body
            .replace("@ATTEMPTS@", &attempts.display().to_string())
            .replace("@MARKER@", &marker.display().to_string())
            .replace("@ARGS@", &args_log.display().to_string())
            .replace("@ANSWERS@", &answers_log.display().to_string())
            .replace("@ABORTED@", &aborted_log.display().to_string());
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
        recorded_lines(&scratch.path(name).with_extension("args"), "arguments")
    }

    /// Answer lines the fake engine read on stdin, in arrival order.
    #[cfg(unix)]
    fn recorded_answers(scratch: &Scratch, name: &str) -> Vec<String> {
        recorded_lines(&scratch.path(name).with_extension("answers"), "answers")
    }

    /// Abort request lines the fake engine received, in arrival order.
    #[cfg(unix)]
    fn recorded_aborts(scratch: &Scratch, name: &str) -> Vec<String> {
        recorded_lines(&scratch.path(name).with_extension("aborted"), "aborts")
    }

    /// Lines a fake engine wrote to its side log, waiting for the child to flush.
    #[cfg(unix)]
    fn recorded_lines(path: &Path, what: &str) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(raw) = std::fs::read_to_string(path)
                && !raw.trim().is_empty()
            {
                return raw.lines().map(|line| line.to_string()).collect();
            }
            assert!(
                Instant::now() < deadline,
                "fake pi never recorded its {what} at {}",
                path.display()
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
    // B1 of `agent-question-bridge.md`: the question extension is loaded with a
    // second `-e`, next to the built-in MCP support, because `--no-extensions`
    // leaves pi without any way to ask the human anything. M2 of
    // `permission-modes.md`: the permission gate is the third `-e`, loaded right
    // after it — a gate nobody loads is a gate that blocks nothing.
    #[test]
    fn spawn_args_match_the_pi_rpc_invocation() {
        let args = spawn_args(
            Path::new("/tmp/sessions/1-1700000000.jsonl"),
            "nan",
            "deepseek-v4-flash",
            SpawnLayout {
                ask_extension: Some(Path::new("/tmp/nabla/extensions/nabla-ask.ts")),
                permissions_extension: Some(Path::new(
                    "/tmp/nabla/extensions/nabla-permissions.ts",
                )),
                data_dir: Some(Path::new("/tmp/nabla")),
            },
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
                "-e".to_string(),
                "/tmp/nabla/extensions/nabla-ask.ts".to_string(),
                "-e".to_string(),
                "/tmp/nabla/extensions/nabla-permissions.ts".to_string(),
                "--append-system-prompt".to_string(),
                NABLA_SYSTEM_PROMPT.to_string(),
            ]
        );
        // The identity actually says who we are: it is not an empty placeholder.
        assert!(NABLA_SYSTEM_PROMPT.contains("Nabla"));
        // And it must never claim the terminal harness's identity.
        assert!(!NABLA_SYSTEM_PROMPT.contains("el Gentleman"));
    }

    // Without a materialized extension the session still has to start: the agent
    // simply has no ask tools and no gate, and the flag list must not carry a
    // dangling `-e`.
    #[test]
    fn spawn_args_omit_the_extra_flag_when_no_extension_was_materialized() {
        let args = spawn_args(
            Path::new("/tmp/1.jsonl"),
            "nan",
            "m",
            SpawnLayout::default(),
        );
        // Only the built-in MCP support is loaded back: one `-e` pair, no second.
        assert_eq!(
            args.iter().filter(|arg| *arg == "-e").count(),
            1,
            "a missing extension must not leave a dangling flag: {args:?}"
        );
        assert_eq!(
            args.iter().filter(|arg| *arg == "builtin:mcp").count(),
            1,
            "the MCP flag rides with its own -e: {args:?}"
        );
        // The identity is the last flag pair either way.
        assert_eq!(args.last(), Some(&NABLA_SYSTEM_PROMPT.to_string()));
    }

    // The browser pane became drivable when the MCP server landed, and this
    // prompt went on claiming the opposite for long enough that the agent
    // answered "no puedo ver esa pestaña" while ten `nabla-browser` tools were
    // connected in the same session. The prompt is the only place that can draw
    // the line between "the drawer is the human's" and "this one tab is yours".
    #[test]
    fn the_identity_tells_the_agent_it_can_drive_the_browser_pane() {
        assert!(
            NABLA_SYSTEM_PROMPT.contains("nabla-browser"),
            "without the server name the agent cannot find its browser tools"
        );
        for tool in ["navigate", "eval", "click", "list_targets"] {
            assert!(
                NABLA_SYSTEM_PROMPT.contains(tool),
                "the identity must name `{tool}`"
            );
        }
        // The sentence that caused the denial. The drawer is still the human's,
        // but stating it as "not yours" also denies the browser tools.
        assert!(
            !NABLA_SYSTEM_PROMPT.contains("not yours"),
            "a blanket claim over the drawer also denies the browser tools"
        );
        // The honest limits stay stated: no screenshot, no other tabs.
        assert!(NABLA_SYSTEM_PROMPT.contains("screenshots"));
        // And what to do when there is nothing to drive. Asserted on a fragment
        // that cannot be split by re-wrapping the file.
        assert!(NABLA_SYSTEM_PROMPT.contains("no view is open"));
    }

    // B1: the tool list a user can only trust if it is really in the extension.
    #[test]
    fn the_question_extension_registers_the_three_ask_tools() {
        for tool in ["ask_select", "ask_confirm", "ask_input"] {
            assert!(
                ASK_EXTENSION_SOURCE.contains(tool),
                "the extension must register `{tool}`"
            );
        }
        // S2: the tools are dialogs that defer to pi's UI sub-protocol, never a
        // paraphrase of it.
        for dialog in ["ctx.ui.select(", "ctx.ui.confirm(", "ctx.ui.input("] {
            assert!(
                ASK_EXTENSION_SOURCE.contains(dialog),
                "the extension must call `{dialog}`"
            );
        }
        // S4: no question carries a timeout, in code or as a field. The word in
        // prose is fine; the `timeout:` field is what pi would honour.
        assert!(
            !ASK_EXTENSION_SOURCE.contains("timeout:"),
            "a question must never carry a timeout (S4)"
        );
        // S2: there is no built-in ask extension, and the identity must never
        // claim otherwise (L3 of the task doc).
        assert!(!ASK_EXTENSION_SOURCE.contains("builtin:ask"));
    }

    // S5: the identity announces the capability, because a capability the prompt
    // never mentions is half a feature (L8 of `webview-control.md`).
    #[test]
    fn the_identity_announces_the_ask_tools() {
        for tool in ["ask_select", "ask_confirm", "ask_input"] {
            assert!(
                NABLA_SYSTEM_PROMPT.contains(tool),
                "the identity must name `{tool}`"
            );
        }
    }

    // B1: the extension is embedded and written to the data dir at boot, so `-e`
    // always points at a file that matches this binary.
    #[test]
    fn materialize_ask_extension_writes_the_embedded_source_idempotently() {
        let scratch = Scratch::new("ask_extension");
        let path = materialize_ask_extension(&scratch.dir).expect("materialize");
        assert_eq!(path, scratch.dir.join("extensions").join("nabla-ask.ts"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            ASK_EXTENSION_SOURCE
        );
        // A second boot answers the same path and rewrites the same bytes.
        let again = materialize_ask_extension(&scratch.dir).expect("materialize again");
        assert_eq!(again, path);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            ASK_EXTENSION_SOURCE
        );
    }

    // M2: the gate is embedded next to the question extension, and a stale copy
    // under the data dir is replaced (an app update must not keep an old gate).
    #[test]
    fn materialize_permissions_extension_writes_the_embedded_source_idempotently() {
        let scratch = Scratch::new("permissions_extension");
        let path = materialize_permissions_extension(&scratch.dir).expect("materialize");
        assert_eq!(
            path,
            scratch.dir.join("extensions").join("nabla-permissions.ts")
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            PERMISSIONS_EXTENSION_SOURCE
        );
        std::fs::write(&path, "// stale gate\n").expect("write stale copy");
        let again = materialize_permissions_extension(&scratch.dir).expect("rematerialize");
        assert_eq!(again, path);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            PERMISSIONS_EXTENSION_SOURCE,
            "a stale gate must be replaced by the embedded one"
        );
    }

    // M2: the gate is only a gate if it really reads the mode per call, asks
    // before a command, and never degrades to `yolo` when it cannot read the
    // projection (L4: the fail-safe direction is `write`).
    #[test]
    fn the_permissions_extension_reads_the_mode_on_every_tool_call() {
        for needle in [
            "pi.on(\"tool_call\"",
            "NABLA_DATA_DIR",
            "permissions.json",
            "ctx.hasUI",
            "ctx.ui.confirm(",
        ] {
            assert!(
                PERMISSIONS_EXTENSION_SOURCE.contains(needle),
                "the gate must contain `{needle}`"
            );
        }
        // The three modes and the four tools S3 names, spelled out.
        for word in ["\"read\"", "\"write\"", "\"yolo\""] {
            assert!(PERMISSIONS_EXTENSION_SOURCE.contains(word), "{word}");
        }
        for tool in ["bash", "powershell", "edit", "write"] {
            assert!(PERMISSIONS_EXTENSION_SOURCE.contains(tool), "{tool}");
        }
        // L4: a read failure answers `write`, never `yolo`.
        assert!(
            !PERMISSIONS_EXTENSION_SOURCE.contains("return \"yolo\""),
            "an unreadable projection must never fall back to yolo"
        );
        // The sibling-preflight effect is documented where it can be read.
        assert!(PERMISSIONS_EXTENSION_SOURCE.contains("preflight"));
    }

    // L4: the child learns WHERE the projection is (`NABLA_DATA_DIR`), never WHAT
    // the mode is — an environment is fixed at spawn time, so a mode in it could
    // not change without restarting the session (S2).
    #[test]
    fn build_command_forwards_the_data_dir_and_never_the_mode() {
        let command = build_command(
            "pi",
            Path::new("/tmp/1.jsonl"),
            "nan",
            "m",
            Path::new("/tmp"),
            SpawnLayout {
                ask_extension: None,
                permissions_extension: Some(Path::new(
                    "/data/extensions/nabla-permissions.ts",
                )),
                data_dir: Some(Path::new("/data")),
            },
        );
        let envs: Vec<(String, String)> = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().to_string(),
                    value
                        .map(|value| value.to_string_lossy().to_string())
                        .unwrap_or_default(),
                )
            })
            .collect();
        let data_dir = envs
            .iter()
            .find(|(name, _)| name == "NABLA_DATA_DIR")
            .expect("the child must learn where the projection lives");
        assert_eq!(data_dir.1, "/data");
        assert!(
            !envs
                .iter()
                .any(|(name, _)| name.to_uppercase().contains("PERMISSION")),
            "the mode itself must never travel in the environment: {envs:?}"
        );
    }

    // B2 end to end: a request from the child reaches the sink typed, and the
    // answer reaches the child's stdin as the exact protocol line.
    #[cfg(unix)]
    #[test]
    fn extension_ui_request_is_forwarded_then_answered_on_stdin() {
        let scratch = Scratch::new("ui_answer");
        let attempts = scratch.path("attempts");
        let marker = scratch.path("marker");
        let script = write_script(&scratch, "fake-pi.sh", &attempts, &marker, FAKE_PI_UI);
        let collected = Arc::new(Collected::default());
        let extension = scratch.path("nabla-ask.ts");
        let manager = ProcessManager::with_timeouts(collected.sink(), test_timeouts())
            .with_ask_extension(extension.clone());

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

        // The extension path reached the child as a second `-e`.
        let args = recorded_args(&scratch, "fake-pi.sh");
        let extension_flag = args
            .iter()
            .position(|arg| arg == extension.to_string_lossy().as_ref());
        let extension_at = extension_flag.expect("-e <path> must reach the child");
        assert_eq!(args.get(extension_at - 1).map(String::as_str), Some("-e"));

        // The question is typed on the way to the webview.
        let events = collected.wait_for("extension ui request", |events| {
            events.iter().any(|(key, event)| {
                key == "s1" && matches!(event, AgentEvent::ExtensionUiRequest { .. })
            })
        });
        let request = events
            .iter()
            .find_map(|(_, event)| match event {
                AgentEvent::ExtensionUiRequest {
                    id,
                    method,
                    title,
                    options,
                    message,
                    placeholder,
                } => Some((
                    id.clone(),
                    method.clone(),
                    title.clone(),
                    options.clone(),
                    message.clone(),
                    placeholder.clone(),
                )),
                _ => None,
            })
            .expect("typed request");
        assert_eq!(request.0, "ui-1");
        assert_eq!(request.1, "select");
        assert_eq!(request.2.as_deref(), Some("Which one?"));
        assert_eq!(request.3, Some(vec!["A".to_string(), "B".to_string()]));
        assert_eq!(request.4, None);
        assert_eq!(request.5, None);

        // The answer reaches the child as one exact line, not as a request.
        manager
            .answer_ui_request("s1", "ui-1", &UiAnswer::Value { value: "A".into() })
            .expect("answer");
        let answers = recorded_answers(&scratch, "fake-pi.sh");
        assert_eq!(
            answers,
            vec![r#"{"type":"extension_ui_response","id":"ui-1","value":"A"}"#.to_string()]
        );

        // A session with no child cannot be answered: it is an error, never silence.
        assert!(matches!(
            manager.answer_ui_request("ghost", "ui-1", &UiAnswer::Cancelled { cancelled: true }),
            Err(ProcessError::NotRunning(_))
        ));
        manager.stop_all();
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

    // S2 of `turn-continuity.md`: a stop goes through pi's own `abort` and waits
    // for the turn to settle, so the session — and the turn it just closed —
    // survive instead of dying with the pipe.
    #[cfg(unix)]
    #[test]
    fn abort_settles_the_turn_and_keeps_the_session_alive() {
        let scratch = Scratch::new("abort_settles");
        let attempts = scratch.path("attempts");
        let marker = scratch.path("marker");
        let script = write_script(&scratch, "fake-pi.sh", &attempts, &marker, FAKE_PI_ABORT);
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

        // A turn really is running, so the abort has something to close.
        let prompt = manager
            .send("s1", "prompt", {
                let mut map = Map::new();
                map.insert("message".to_string(), json!("hi"));
                map
            })
            .expect("prompt response");
        assert!(prompt.success);
        assert_eq!(manager.status("s1").map(|s| s.status), Some(RuntimeStatus::Running));

        let started = Instant::now();
        let outcome = manager.abort_and_wait("s1").expect("abort");
        let waited = started.elapsed();

        assert_eq!(outcome, AbortOutcome::Settled, "pi closed the turn itself");
        assert!(
            waited < test_timeouts().abort_grace,
            "a settled abort returns inside the grace instead of waiting it out: {waited:?}"
        );
        // S2: the session survives, addressable, with no kill behind the stop.
        assert!(
            manager.contains("s1"),
            "a settled stop must not unregister the session"
        );
        assert_eq!(
            manager.status("s1").map(|s| s.status),
            Some(RuntimeStatus::Ready),
            "the process is idle again, not stopped"
        );
        let events = collected.snapshot();
        assert!(
            !events
                .iter()
                .any(|(_, event)| matches!(event, AgentEvent::ProcessExit { .. })),
            "nothing was killed, so nothing exited: {events:#?}"
        );
        // The turn ended through the protocol, with the reason the UI needs.
        assert!(
            events.iter().any(|(key, event)| key == "s1"
                && matches!(event, AgentEvent::TurnEnd { stop_reason: Some(StopReason::Aborted), .. })),
            "the aborted turn is forwarded with its reason: {events:#?}"
        );
        // The request reached the child as the protocol's own frame, once, with
        // our id and no empty payload fields.
        let aborts = recorded_aborts(&scratch, "fake-pi.sh");
        assert_eq!(aborts.len(), 1, "exactly one abort frame: {aborts:?}");
        let frame: Value = serde_json::from_str(&aborts[0]).expect("one JSON line");
        assert_eq!(frame["type"], json!("abort"));
        assert_eq!(
            frame["id"],
            json!("2"),
            "the id is ours, minted like every other request"
        );
        assert_eq!(
            frame.as_object().map(Map::len),
            Some(2),
            "nothing but the type and the id travels: {}",
            aborts[0]
        );

        // And the session still takes work: the same child answers a new turn.
        let again = manager
            .send("s1", "prompt", {
                let mut map = Map::new();
                map.insert("message".to_string(), json!("again"));
                map
            })
            .expect("the session must stay usable after a settled stop");
        assert!(again.success);
        assert_eq!(attempts_count(&attempts), 1, "the stop never respawned the child");
        manager.stop_all();
    }

    // S2: the fallback is today's behaviour, unchanged. A child that ignores the
    // abort is killed after the bounded wait, unregistered, and reported Stopped
    // with its synthetic exit — exactly what a stop did before this feature.
    #[cfg(unix)]
    #[test]
    fn a_child_that_never_settles_is_killed_after_the_grace() {
        let scratch = Scratch::new("abort_deaf");
        let attempts = scratch.path("attempts");
        let marker = scratch.path("marker");
        let script = write_script(&scratch, "fake-pi.sh", &attempts, &marker, FAKE_PI_ABORT_DEAF);
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

        let started = Instant::now();
        let outcome = manager.abort_and_wait("s1").expect("abort");
        let waited = started.elapsed();

        assert_eq!(outcome, AbortOutcome::Killed, "a child that never settles is killed");
        assert!(
            waited >= test_timeouts().abort_grace,
            "the abort is given its whole grace before the kill: {waited:?}"
        );
        assert_eq!(
            recorded_aborts(&scratch, "fake-pi.sh").len(),
            1,
            "the abort was really sent before the kill"
        );
        assert!(
            !manager.contains("s1"),
            "a killed session is no longer registered"
        );
        assert_eq!(manager.status("s1"), None);
        let events = collected.wait_for("stopped status", |events| {
            statuses(events).contains(&RuntimeStatus::Stopped)
        });
        assert!(
            events.iter().any(|(key, event)| key == "s1"
                && matches!(event, AgentEvent::ProcessExit { code: Some(0), .. })),
            "the kill still reports the synthetic exit: {events:#?}"
        );
    }

    // S2 keeps `stop_and_wait`'s contract on this new path too: a stop for a key
    // nothing runs under is nothing to abort and nothing to kill, not an error.
    #[test]
    fn abort_without_a_session_reports_nothing_running() {
        let collected = Arc::new(Collected::default());
        let manager = ProcessManager::with_timeouts(collected.sink(), test_timeouts());
        assert_eq!(
            manager.abort_and_wait("ghost").expect("idempotent"),
            AbortOutcome::NotRunning
        );
        assert!(
            collected.snapshot().is_empty(),
            "nothing to stop, nothing announced"
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
