// CLI bridge module: a generic, allowlisted bridge to the `pi` command line
// (spec S1 of `pi-cli-bridge.md`) plus the interactive session bridge the OAuth
// device-code login needs (S3).
//
// Design notes:
// - argv only: `std::process::Command` with a typed argument vector, never a
//   shell, so there is no interpolation surface at all (S1, S2).
//! - the allowlist is enforced in Rust before anything is spawned: the FIRST
//!   argument must be one of `ALLOWED_SUBCOMMANDS` (S2). `install`/`update` are
//!   outside it because they run third-party code, `auth print-api-key` /
//!   `auth print-bearer-token` are never called from here because they would hand
//!   secrets to the webview, and `mcp` is inside it because F0 of
//!   `tool-drawer-and-mcp.md` configures MCP servers through pi's own CLI.
// - the child inherits the same environment allowlist the pi runtime forwards
//   (`process::forwarded_env`): this bridge can never leak the host environment.
// - output is bounded: each non-interactive stream is capped at `STREAM_CAP`
//   bytes and reported as `truncated` instead of growing without limit (S2).
// - no busy loop: `run` waits on a channel fed by a waiter thread, and a timeout
//   terminates the child's whole process group (the same kill path the runtime
//   uses), escalating from the polite signal to the force kill.
// - two process models, on purpose: `run` is a batch call on plain pipes, while an
//   interactive session runs on a real pseudo-terminal (`portable-pty`: a Unix pty
//   or a Windows ConPTY under the same API). `pi auth login` refuses to start
//   without a TTY, and its OAuth flow waits at a prompt for a pasted redirect URL.
//   Terminal output is split into lines and stripped of ANSI before it reaches the
//   webview, which is a log view and not a terminal emulator (defect L6).
// - `auth_check_many` is the batch form of the account probe: one `pi auth check`
//   boots a whole CLI, so asking six providers one after another costs ~7 s. The
//   batch runs them through a bounded pool (`AUTH_CHECK_POOL`) with the same
//   per-provider deadline, keeps request order, and answers `unknown` per entry
//   instead of failing the call (S2 of `providers-pane-refinements.md`).
//
// Frozen contract (see `odd/tasks/pi-cli-bridge.md`): `PiCliResult` is the
// non-interactive answer, `CliEvent` is the `pi-cli` event payload, and
// `PiAuthStatus` is the typed account state. All of them serialize camelCase so
// the TS contracts (C3) mirror them field by field.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::iter::Peekable;
use std::process::{Command, Stdio};
use std::str::Chars;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{ChildKiller, CommandBuilder, PtyPair, PtySize, native_pty_system};

/// Cap of each captured stream in `run`, in bytes (S2: "salida acotada", 64 KiB
/// per stream). Interactive sessions stream line by line instead, so they are not
/// capped.
pub const STREAM_CAP: usize = 64 * 1024;

/// Default deadline of a non-interactive call (S2: "timeout por defecto 20s").
pub const DEFAULT_TIMEOUT_MS: u64 = 20_000;

/// Grace given to a child to leave after the polite terminate signal before the
/// force kill. Bounds the extra wait a timed-out `run` can cost.
pub const KILL_GRACE: Duration = Duration::from_millis(500);

/// Grace given to a child to leave after `stop` terminates it.
pub const STOP_GRACE: Duration = Duration::from_millis(500);

/// How many `pi auth check` probes may run at once (S2 of
/// `providers-pane-refinements.md`). Each probe boots a whole CLI, so the cost of
/// six providers is dominated by how many waves they need: four in flight turns
/// six probes into two waves. Bounded on purpose — a pool as wide as the provider
/// list would spawn as many CLIs as the file happens to name.
pub const AUTH_CHECK_POOL: usize = 4;

/// Geometry of an interactive session's pty. Wide on purpose: `pi auth login`
/// prints a long authorization URL, and a child that wraps to the terminal width
/// would split it across two lines for no reason. Nothing renders this width; it
/// only tells the child how much room it has.
pub const PTY_ROWS: u16 = 50;
pub const PTY_COLS: u16 = 200;

/// The subcommands this bridge is allowed to run (S2). The first argument of
/// every call must be exactly one of these: nothing is matched by prefix, so
/// `auth print-api-key` can only ever run if `auth` is allowed — and Nabla never
/// asks for it.
pub const ALLOWED_SUBCOMMANDS: &[&str] =
    &["auth", "models", "config", "mcp", "--version", "--help"];

/// Answer of one non-interactive call. Mirrors the frozen contract:
/// `{ stdout, stderr, exitCode, timedOut, truncated }` (S1, S2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiCliResult {
    pub stdout: String,
    pub stderr: String,
    /// `None` when the child was terminated by a signal (no exit code exists).
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub truncated: bool,
}

/// Account state reported by `pi auth check --json` (S4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    Ready,
    NotReady,
    Invalid,
    Unknown,
}

impl AuthState {
    /// Words pi (or a future pi) may use for each state. Compared lowercased, so
    /// `Not-Ready` and `not_ready` are the same answer.
    fn parse(raw: &str) -> Option<AuthState> {
        let normalized = raw.trim().to_ascii_lowercase().replace('-', "_");
        match normalized.as_str() {
            "ready" | "ok" | "logged_in" | "authenticated" | "valid" => Some(AuthState::Ready),
            "not_ready" | "notready" | "logged_out" | "unauthenticated" | "missing" => {
                Some(AuthState::NotReady)
            }
            "invalid" | "invalid_credential" | "expired" | "error" => Some(AuthState::Invalid),
            "unknown" => Some(AuthState::Unknown),
            _ => None,
        }
    }

    /// Documented exit codes of `pi auth check`: 0 ready, 1 not ready, 2 invalid.
    /// Used ONLY as a fallback when the JSON carries no status field.
    fn from_exit_code(code: i32) -> AuthState {
        match code {
            0 => AuthState::Ready,
            1 => AuthState::NotReady,
            2 => AuthState::Invalid,
            _ => AuthState::Unknown,
        }
    }
}

/// Typed account state (S4). `credential` says whether a credential is present,
/// never its value: the bridge must not move secrets into the webview.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiAuthStatus {
    pub provider: String,
    pub state: AuthState,
    pub credential: bool,
}

impl PiAuthStatus {
    /// What an entry answers when its probe produced nothing usable: a timeout, a
    /// child that could not run, a blank provider id. Never a guess about the
    /// account, and never a reason to fail a whole batch.
    fn unknown(provider: &str) -> Self {
        PiAuthStatus {
            provider: provider.to_string(),
            state: AuthState::Unknown,
            credential: false,
        }
    }
}

/// The three streams one interactive session reports (S3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CliStream {
    Stdout,
    Stderr,
    Exit,
}

/// Payload of the Tauri `pi-cli` event: `{ cliSessionId, stream, line?, exitCode? }`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliEvent {
    pub cli_session_id: String,
    pub stream: CliStream,
    /// One complete line, without its trailing newline. Absent on `exit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// Exit code of the child. Present only on `exit`, and only when the child
    /// left on its own rather than being killed by a signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// Handle of one interactive session, as handed to the webview (S3).
pub type CliSessionId = String;

/// Where every session event goes. The command layer turns this into the Tauri
/// `pi-cli` event; tests collect it in a vector.
pub type CliSink = Arc<dyn Fn(&CliEvent) + Send + Sync>;

/// Failures of the bridge. Plain strings cross the Tauri bridge (see
/// commands.rs); this enum keeps them typed and testable inside Rust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// No arguments at all: there is no subcommand to allow.
    EmptyArgs,
    /// The first argument is not in `ALLOWED_SUBCOMMANDS` (S2).
    NotAllowed(String),
    /// The child could not be started at all.
    Spawn(String),
    /// Writing to a live child's stdin failed.
    Write(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::EmptyArgs => write!(
                f,
                "pi cli: empty argument list; the first argument must be one of {}",
                ALLOWED_SUBCOMMANDS.join(", ")
            ),
            CliError::NotAllowed(arg) => write!(
                f,
                "pi cli: subcommand not allowed: {arg} (allowed: {})",
                ALLOWED_SUBCOMMANDS.join(", ")
            ),
            CliError::Spawn(message) => write!(f, "pi cli: {message}"),
            CliError::Write(message) => write!(f, "pi cli: stdin write failed: {message}"),
        }
    }
}

impl std::error::Error for CliError {}

/// Runs one allowlisted `pi` invocation and captures its output (S1, S2).
///
/// A non-zero exit is an answer, not an error: the caller decides what a code
/// means. `Err` is reserved for a call that could not run at all.
pub fn run(args: &[String], timeout: Duration, binary: &str) -> Result<PiCliResult, CliError> {
    check_allowlist(args)?;
    // A zero deadline would kill the child before it could even read its argv.
    let timeout = timeout.max(Duration::from_millis(1));
    let mut child = build_command(binary, args)
        .spawn()
        .map_err(|err| CliError::Spawn(format!("failed to spawn {binary}: {err}")))?;
    let pid = child.id();
    let stdout_reader = child
        .stdout
        .take()
        .map(|pipe| thread::spawn(move || read_capped(pipe)));
    let stderr_reader = child
        .stderr
        .take()
        .map(|pipe| thread::spawn(move || read_capped(pipe)));

    // The waiter owns the handle, so the deadline is enforced by a channel and
    // never by polling: no busy loop can spin here (S2).
    let (exit_tx, exit_rx) = mpsc::channel::<Option<i32>>();
    let waiter = thread::spawn(move || {
        let code = child.wait().ok().and_then(|status| status.code());
        let _ = exit_tx.send(code);
    });

    let (exit_code, timed_out) = match exit_rx.recv_timeout(timeout) {
        Ok(code) => (code, false),
        // A child that outlives its deadline is killed, never left behind (S2).
        Err(_) => (terminate(pid, &exit_rx), true),
    };
    let _ = waiter.join();

    let (stdout, stdout_cut) = join_stream(stdout_reader);
    let (stderr, stderr_cut) = join_stream(stderr_reader);
    Ok(PiCliResult {
        stdout,
        stderr,
        exit_code,
        timed_out,
        truncated: stdout_cut || stderr_cut,
    })
}

/// Parses the JSON of `pi auth check --json` (S4). Never panics: anything it
/// cannot understand is reported as `unknown` with no credential.
pub fn parse_auth_check(json: &str) -> PiAuthStatus {
    interpret_auth_json("", json, None)
}

/// Parses the JSON of `pi auth check --json`, falling back to the documented
/// exit codes when the JSON carries no status field (S4).
pub fn auth_status(provider: &str, json: &str, exit_code: Option<i32>) -> PiAuthStatus {
    // A child killed by a signal reports no code, so the JSON is the only
    // evidence left: that is exactly what the contract's parser reads.
    if exit_code.is_none() {
        let mut status = parse_auth_check(json);
        if status.provider.trim().is_empty() {
            status.provider = provider.to_string();
        }
        return status;
    }
    interpret_auth_json(provider, json, exit_code)
}

/// Exact argv of the account probe (S4): `pi auth check --provider <p> --json`.
/// Typed arguments, so the provider name can never be read as a flag or a shell
/// fragment.
pub fn auth_check_args(provider: &str) -> Vec<String> {
    vec![
        "auth".to_string(),
        "check".to_string(),
        "--provider".to_string(),
        provider.to_string(),
        "--json".to_string(),
    ]
}

/// Maps one finished probe to the typed account state (S4): a probe that ran out
/// of time says nothing about the account, so it answers `unknown` instead of
/// inventing a login state. Both the single and the batch command share this
/// mapping, so they can never disagree about what a timeout means.
pub fn auth_status_from_result(provider: &str, result: &PiCliResult) -> PiAuthStatus {
    if result.timed_out {
        return PiAuthStatus::unknown(provider);
    }
    auth_status(provider, &result.stdout, result.exit_code)
}

/// S2 of `providers-pane-refinements.md`: the typed account state of many
/// providers in one call, probed concurrently.
///
/// One `pi auth check` boots a whole CLI (~1.15 s measured), so six providers
/// asked one after another cost ~7 s. The pool below caps the probes in flight at
/// `AUTH_CHECK_POOL`, which turns that into two waves instead of six spawns; the
/// deadline stays per provider (never a shared batch budget), so a slow provider
/// costs its own timeout and nothing else.
///
/// The answer has one entry per requested provider, in request order: the UI maps
/// by provider id, but a stable order keeps the batch honest and testable. It
/// cannot fail per batch — a provider that times out, answers garbage or cannot
/// even be spawned answers `unknown` for its own slot and never aborts the rest.
pub fn auth_check_many(providers: &[String], timeout: Duration, binary: &str) -> Vec<PiAuthStatus> {
    // Every slot starts `unknown`, so a probe only ever replaces its own entry.
    let mut statuses: Vec<PiAuthStatus> = providers
        .iter()
        .map(|provider| PiAuthStatus::unknown(provider))
        .collect();
    // A blank id names no provider, so it is never handed to `pi`: it keeps its
    // slot and its id, and stays `unknown`.
    let jobs: Arc<Vec<usize>> = Arc::new(
        providers
            .iter()
            .enumerate()
            .filter(|(_, provider)| !provider.trim().is_empty())
            .map(|(index, _)| index)
            .collect(),
    );
    if jobs.is_empty() {
        return statuses;
    }

    // A pool wider than the work would just start threads that immediately find
    // nothing to claim.
    let workers = jobs.len().min(AUTH_CHECK_POOL.max(1));
    let providers: Arc<Vec<String>> = Arc::new(providers.to_vec());
    let binary: Arc<str> = Arc::from(binary);
    let next = Arc::new(AtomicUsize::new(0));
    let (results_tx, results_rx) = mpsc::channel::<(usize, PiAuthStatus)>();

    let mut workers_handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let jobs = Arc::clone(&jobs);
        let providers = Arc::clone(&providers);
        let binary = Arc::clone(&binary);
        let next = Arc::clone(&next);
        let results_tx = results_tx.clone();
        workers_handles.push(thread::spawn(move || {
            // Each worker claims the next unclaimed slot, so a slow provider idles
            // at most one worker and no slot is probed twice: no barrier to wait
            // on, and no fixed partition that could strand the tail of the list.
            while let Some(&index) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                let status = probe_auth(&providers[index], timeout, &binary);
                if results_tx.send((index, status)).is_err() {
                    break;
                }
            }
        }));
    }
    drop(results_tx);

    // Collected on the calling thread: the batch is over when every slot's probe
    // has answered, and a lost worker can never drop a result silently.
    for (index, status) in results_rx {
        statuses[index] = status;
    }
    for handle in workers_handles {
        let _ = handle.join();
    }
    statuses
}

/// One probe of the batch. A probe that cannot even run is `unknown` for its own
/// entry only: a broken provider must never fail the whole batch.
fn probe_auth(provider: &str, timeout: Duration, binary: &str) -> PiAuthStatus {
    match run(&auth_check_args(provider), timeout, binary) {
        Ok(result) => auth_status_from_result(provider, &result),
        Err(_) => PiAuthStatus::unknown(provider),
    }
}

/// One reading of `pi auth check --json`.
///
/// A document that is not a JSON object says nothing about the account, so it
/// answers `unknown` (S4) — the exit-code fallback applies only to an object
/// without a usable status field, never to garbage that happened to exit 0.
fn interpret_auth_json(provider: &str, json: &str, exit_code: Option<i32>) -> PiAuthStatus {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(json) else {
        return PiAuthStatus {
            provider: provider.to_string(),
            state: AuthState::Unknown,
            credential: false,
        };
    };
    let reported_provider = ["provider", "providerId"]
        .iter()
        .find_map(|key| map.get(*key).and_then(|value| value.as_str()))
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| provider.to_string());
    let state = ["status", "state"]
        .iter()
        .find_map(|key| {
            map.get(*key)
                .and_then(|value| value.as_str())
                .and_then(AuthState::parse)
        })
        .or_else(|| {
            ["loggedIn", "logged_in", "authenticated"]
                .iter()
                .find_map(|key| map.get(*key).and_then(|value| value.as_bool()))
                .map(|logged_in| match logged_in {
                    true => AuthState::Ready,
                    false => AuthState::NotReady,
                })
        })
        .or_else(|| exit_code.map(AuthState::from_exit_code))
        .unwrap_or(AuthState::Unknown);
    let credential = ["credential", "hasCredential", "has_credential"]
        .iter()
        .find_map(|key| map.get(*key).and_then(|value| value.as_bool()))
        .unwrap_or(state == AuthState::Ready);
    PiAuthStatus {
        provider: reported_provider,
        state,
        credential,
    }
}

/// Reads one pipe up to `STREAM_CAP`, draining the rest so a chatty child is
/// never blocked on a full pipe, and reports whether anything was cut (S2).
fn read_capped<R: Read>(mut pipe: R) -> (String, bool) {
    let mut captured: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(filled) => {
                let room = STREAM_CAP.saturating_sub(captured.len());
                let keep = filled.min(room);
                captured.extend_from_slice(&chunk[..keep]);
                if keep < filled {
                    truncated = true;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            // A read error on a pipe that was already drained is not worth
            // failing the whole call: what was captured is what the caller gets.
            Err(_) => break,
        }
    }
    // Decoded lossily: a cut can land inside a multi-byte character.
    (String::from_utf8_lossy(&captured).into_owned(), truncated)
}

/// Joins one stream reader, treating a thread that never produced bytes as an
/// empty stream instead of panicking the command layer.
fn join_stream(handle: Option<thread::JoinHandle<(String, bool)>>) -> (String, bool) {
    match handle.map(thread::JoinHandle::join) {
        Some(Ok(captured)) => captured,
        _ => (String::new(), false),
    }
}

/// Escalating kill of a timed-out child's whole process group: the polite
/// signal first, the force kill only when the child ignores it (S2).
fn terminate(pid: u32, exit: &mpsc::Receiver<Option<i32>>) -> Option<i32> {
    crate::process::signal_terminate(pid);
    match exit.recv_timeout(KILL_GRACE) {
        Ok(code) => code,
        Err(_) => {
            crate::process::signal_kill(pid);
            exit.recv_timeout(KILL_GRACE).unwrap_or(None)
        }
    }
}

/// Removes terminal control sequences from one line of pty output (S3).
///
/// A terminal child (pi's login flow) emits colors, cursor moves and a window
/// title on top of the text the webview has to show. The webview is a log view,
/// not a terminal emulator, so the sequences are stripped here instead of being
/// forwarded (`ESC[1G` would otherwise render as garbage).
///
/// Kept inline and dependency-free: the rules are small, and a crate would only
/// add a version to pin for four escape forms.
pub fn strip_ansi(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            // ESC introduces a control sequence: CSI (`ESC [`), OSC (`ESC ]`) or
            // a two-byte escape with an optional intermediate byte.
            '\u{1b}' => match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    skip_csi(&mut chars);
                }
                Some(']') => {
                    chars.next();
                    skip_osc(&mut chars);
                }
                // Charset designators (`ESC ( B`) carry one intermediate byte.
                Some('(' | ')' | '*' | '+' | '#' | '%') => {
                    chars.next();
                    chars.next();
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            // The C1 single-byte forms of the same sequences.
            '\u{9b}' => skip_csi(&mut chars),
            '\u{9d}' => skip_osc(&mut chars),
            // A carriage return redraws from column 0, so what was collected for
            // this line is being overwritten.
            '\r' => out.clear(),
            // A backspace erases one visible character.
            '\u{8}' => {
                out.pop();
            }
            '\t' => out.push('\t'),
            // Every other control character (BEL, NUL, ...) is not text.
            other if other.is_control() => {}
            other => out.push(other),
        }
    }
    out
}

/// Consumes one CSI sequence: parameters, then intermediates, then the final
/// byte that ends it (`m`, `G`, `K`, `J`, `H`, ...). A sequence cut by the end of
/// the line consumes the rest of the line, which is the only reading that cannot
/// leak half a color code into the text.
fn skip_csi(chars: &mut Peekable<Chars<'_>>) {
    for ch in chars.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&ch) {
            return;
        }
    }
}

/// Consumes one OSC string, which ends at BEL or at the two-byte ST (`ESC \`).
fn skip_osc(chars: &mut Peekable<Chars<'_>>) {
    while let Some(ch) = chars.next() {
        match ch {
            '\u{7}' => return,
            '\u{1b}' if chars.peek() == Some(&'\\') => {
                chars.next();
                return;
            }
            _ => {}
        }
    }
}

/// Splits pty output into complete lines (S3).
#[derive(Default)]
pub struct LineSplitter {
    pending: Vec<u8>,
}

impl LineSplitter {
    /// Feeds one chunk of pty bytes and answers the lines it completed. The
    /// chunk boundary is invisible to the caller: a line cut in two is rejoined
    /// here, never emitted twice.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in chunk {
            match byte {
                // The pty turns every newline a child writes into `\r\n`, and a
                // bare `\r` is a redraw: all three end a line here.
                b'\n' | b'\r' => self.finish(&mut lines),
                other => self.pending.push(other),
            }
        }
        lines
    }

    /// Answers the last line, which a closed pty delivered without a terminator.
    pub fn flush(&mut self) -> Option<String> {
        let mut lines = Vec::new();
        if !self.pending.is_empty() {
            self.finish(&mut lines);
        }
        lines.into_iter().next()
    }

    fn finish(&mut self, lines: &mut Vec<String>) {
        let raw = std::mem::take(&mut self.pending);
        // Decoded lossily: a cut can land inside a multi-byte character.
        let line = strip_ansi(&String::from_utf8_lossy(&raw));
        // A line with nothing visible left is not a row in the webview.
        if !line.trim().is_empty() {
            lines.push(line);
        }
    }
}

/// Registry of live interactive sessions (S3). Owned by `AppState`.
#[derive(Default)]
pub struct CliSessions {
    sessions: Mutex<HashMap<CliSessionId, Arc<CliSession>>>,
    next_id: AtomicUsize,
}

impl CliSessions {
    pub fn new() -> CliSessions {
        CliSessions::default()
    }

    /// Spawns one allowlisted child on a pseudo-terminal and starts streaming its
    /// output (S3).
    ///
    /// A pty is not a nicety here: `pi auth login` refuses to run without a TTY
    /// (`requires an interactive terminal`, exit 1, no URL), which is exactly the
    /// defect this bridge exists to avoid. The session is registered before its
    /// reader threads start, so the first event can never arrive for an id nobody
    /// knows yet.
    pub fn start(
        &self,
        args: &[String],
        binary: &str,
        sink: CliSink,
    ) -> Result<CliSessionId, CliError> {
        check_allowlist(args)?;
        let PtyPair { slave, master } = native_pty_system()
            .openpty(PtySize {
                rows: PTY_ROWS,
                cols: PTY_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| CliError::Spawn(format!("failed to open a pty: {err}")))?;
        let mut child = slave
            .spawn_command(pty_command(binary, args))
            .map_err(|err| CliError::Spawn(format!("failed to spawn {binary}: {err}")))?;
        // The slave end must not stay open in this process: the reader only sees
        // the terminal hang up when the last slave descriptor is gone. The pair
        // already drops it first, and this makes the intent explicit.
        drop(slave);

        let pid = child.process_id();
        let reader = master
            .try_clone_reader()
            .map_err(|err| CliError::Spawn(format!("failed to read the pty: {err}")))?;
        let writer = master
            .take_writer()
            .map_err(|err| CliError::Spawn(format!("failed to write to the pty: {err}")))?;
        let killer = child.clone_killer();
        let id = self.next_session_id();
        let exited = Arc::new(AtomicBool::new(false));
        lock(&self.sessions).insert(
            id.clone(),
            Arc::new(CliSession {
                pid,
                writer: Mutex::new(Some(writer)),
                killer: Mutex::new(Some(killer)),
                exited: Arc::clone(&exited),
            }),
        );

        stream_pty(reader, id.clone(), Arc::clone(&sink));

        // Exit watcher: it owns the `Child`, so it can reap the process without
        // holding any lock the writer needs.
        let exit_id = id.clone();
        thread::spawn(move || {
            let code = child.wait().ok().as_ref().and_then(exit_code_of);
            exited.store(true, Ordering::SeqCst);
            sink(&CliEvent {
                cli_session_id: exit_id,
                stream: CliStream::Exit,
                line: None,
                exit_code: code,
            });
        });
        Ok(id)
    }

    /// Writes `text` plus a newline to the session's terminal. `Ok(false)` for an
    /// unknown session id, so a late write is not an error.
    pub fn write(&self, session_id: &str, text: &str) -> Result<bool, CliError> {
        let Some(session) = lock(&self.sessions).get(session_id).cloned() else {
            return Ok(false);
        };
        // A child that already exited has nothing left to read.
        if session.exited.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let mut guard = lock(&session.writer);
        let Some(master) = guard.as_mut() else {
            return Ok(false);
        };
        // A terminal line ends in `\n`: the pty turns it into the carriage return
        // the child sees, so a pasted redirect URL arrives exactly as one line.
        // A URL pasted with its own line ending is collapsed instead of sending an
        // empty second line the child would read as an empty answer.
        let text = text.trim_end_matches(['\r', '\n']);
        let mut line = String::with_capacity(text.len() + 1);
        line.push_str(text);
        line.push('\n');
        master
            .write_all(line.as_bytes())
            .and_then(|()| master.flush())
            .map_err(|err| CliError::Write(err.to_string()))?;
        Ok(true)
    }

    /// Kills the session's child if it is still alive and removes the session.
    /// `Ok(false)` for an unknown session id, so stop is idempotent.
    pub fn stop(&self, session_id: &str) -> Result<bool, CliError> {
        let Some(session) = lock(&self.sessions).remove(session_id) else {
            return Ok(false);
        };
        // Close the terminal first: a child that reads it sees EOF and may leave
        // on its own before any signal is needed.
        lock(&session.writer).take();
        if session.exited.load(Ordering::SeqCst) {
            return Ok(true);
        }
        terminate_session(&session);
        if !wait_exit(&session.exited, STOP_GRACE) {
            // A child that ignores the polite signal must not outlive the host.
            force_kill_session(&session);
            let _ = wait_exit(&session.exited, STOP_GRACE);
        }
        Ok(true)
    }

    /// Stops every live session (S1: no pi child outlives the host).
    pub fn stop_all(&self) {
        let ids: Vec<CliSessionId> = lock(&self.sessions).keys().cloned().collect();
        for id in ids {
            let _ = self.stop(&id);
        }
    }

    /// Ids of the live sessions, sorted so the answer is stable across calls.
    #[cfg(test)]
    fn live_ids(&self) -> Vec<CliSessionId> {
        let mut ids: Vec<CliSessionId> = lock(&self.sessions).keys().cloned().collect();
        ids.sort();
        ids
    }

    fn next_session_id(&self) -> CliSessionId {
        let sequence = self.next_id.fetch_add(1, Ordering::SeqCst);
        format!("cli-{}-{sequence}", std::process::id())
    }
}

impl Drop for CliSessions {
    fn drop(&mut self) {
        // S1: a login flow must not outlive the window that started it.
        self.stop_all();
    }
}

/// Forwards pty output as `line` events until the terminal closes (S3).
///
/// Everything is reported as `stdout`: a terminal merges the child's stderr into
/// the one stream it renders, so a separate `stderr` channel would be a fiction.
/// The ANSI a login flow emits is stripped before it leaves this thread, because
/// the webview is a log view and not a terminal emulator.
fn stream_pty<R: Read + Send + 'static>(reader: R, id: CliSessionId, sink: CliSink) {
    thread::spawn(move || {
        let mut reader = reader;
        let mut splitter = LineSplitter::default();
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(filled) => emit_lines(splitter.push(&chunk[..filled]), &id, &sink),
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                // A pty whose child is gone reports EIO on some platforms and a
                // clean end of file on others: both mean the session is over.
                Err(_) => break,
            }
        }
        // A last line without a terminator is still a line.
        if let Some(last) = splitter.flush() {
            sink(&CliEvent {
                cli_session_id: id,
                stream: CliStream::Stdout,
                line: Some(last),
                exit_code: None,
            });
        }
    });
}

/// Emits one `stdout` event per completed line.
fn emit_lines(lines: Vec<String>, id: &CliSessionId, sink: &CliSink) {
    for line in lines {
        sink(&CliEvent {
            cli_session_id: id.clone(),
            stream: CliStream::Stdout,
            line: Some(line),
            exit_code: None,
        });
    }
}

/// Waits for the exit latch within `budget`. Polled, never spun: the interval is
/// a sleep, and the budget bounds the whole wait.
fn wait_exit(exited: &AtomicBool, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if exited.load(Ordering::SeqCst) {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    exited.load(Ordering::SeqCst)
}

/// Rejects a call the allowlist does not cover, before anything is spawned (S2).
fn check_allowlist(args: &[String]) -> Result<(), CliError> {
    let Some(first) = args.first() else {
        return Err(CliError::EmptyArgs);
    };
    if ALLOWED_SUBCOMMANDS.contains(&first.as_str()) {
        Ok(())
    } else {
        Err(CliError::NotAllowed(first.clone()))
    }
}

/// Builds the child of an interactive session. The pty supplies its stdio on
/// both platforms (a Unix pty pair, a Windows ConPTY), so only the argv and the
/// environment are configured here.
fn pty_command(binary: &str, args: &[String]) -> CommandBuilder {
    let resolved = crate::pi_runtime::resolve_pi(binary);
    let mut command = CommandBuilder::new(&resolved.program);
    command.args(&resolved.prefix_args);
    command.args(args);
    // The same environment allowlist `run` and the pi runtime use: a login flow
    // can never read more of the host environment than the runtime may.
    command.env_clear();
    for (name, value) in crate::process::forwarded_env(std::env::vars()) {
        command.env(name, value);
    }
    if let Some(node_path) = resolved.node_path {
        command.env("NODE_PATH", node_path);
    }
    if let Some(agent_dir) = crate::pi_runtime::agent_dir() {
        command.env("PI_CODING_AGENT_DIR", agent_dir);
    }
    command
}

/// Builds the non-interactive child: piped stdio, allowlisted env, own process
/// group on Unix (so a kill reaches the whole tree). Every mainstream OS is
/// covered by the shared branches; only the platform extras are conditional.
fn build_command(binary: &str, args: &[String]) -> Command {
    let resolved = crate::pi_runtime::resolve_pi(binary);
    let mut command = Command::new(&resolved.program);
    command.args(&resolved.prefix_args);
    command.args(args);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.env_clear();
    for (name, value) in crate::process::forwarded_env(std::env::vars()) {
        command.env(name, value);
    }
    if let Some(node_path) = resolved.node_path {
        command.env("NODE_PATH", node_path);
    }
    if let Some(agent_dir) = crate::pi_runtime::agent_dir() {
        command.env("PI_CODING_AGENT_DIR", agent_dir);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group (pgid == child pid) makes the kill path reach the
        // child AND every descendant it spawned (S2, S3).
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // S1: no console window, ever.
        // SAFETY: a documented creation flag, no memory or pointer effects.
        unsafe {
            command.creation_flags(crate::process::CREATE_NO_WINDOW);
        }
    }
    command
}

/// One managed interactive child and the handles its threads need.
struct CliSession {
    /// Session leader of the pty, when the platform reports one.
    pid: Option<u32>,
    /// Taken by `stop` to close the terminal; a live session always has it.
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    /// The pty's own kill path, used only when no pid is available.
    killer: Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>,
    /// Set by the waiter thread once the child is reaped.
    exited: Arc<AtomicBool>,
}

/// Exit code of a pty child, or `None` when a signal ended it (no code exists).
fn exit_code_of(status: &portable_pty::ExitStatus) -> Option<i32> {
    match status.signal() {
        Some(_) => None,
        None => Some(status.exit_code() as i32),
    }
}

/// Polite termination of one interactive child: the shared process-group signal
/// when the pty named a session leader (so pi's descendants go too), the pty
/// handle otherwise.
fn terminate_session(session: &CliSession) {
    match session.pid {
        Some(pid) => crate::process::signal_terminate(pid),
        None => kill_with_handle(session),
    }
}

/// Last resort for a child that ignores the polite signal.
fn force_kill_session(session: &CliSession) {
    match session.pid {
        Some(pid) => crate::process::signal_kill(pid),
        None => kill_with_handle(session),
    }
}

/// Kills through the pty's own handle, which is the only option when the
/// platform did not report a pid.
fn kill_with_handle(session: &CliSession) {
    let mut guard = lock(&session.killer);
    if let Some(killer) = guard.as_mut() {
        let _ = killer.kill();
    }
}

/// Locks without panicking: a poisoned lock in a background thread must not turn
/// into a session the host can no longer address.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    /// Scratch directory in the system temp dir (never the worktree), removed on
    /// drop.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let mut dir = std::env::temp_dir();
            dir.push(format!(
                "nabla_cli_{}_{}_{tag}",
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

    // ── fake `pi` scripts: no real pi, no network ───────────────────────────

    /// Happy path: one stdout line, one stderr line, exit 0.
    const FAKE_OK: &str = r#"#!/bin/sh
printf 'hello from pi\n'
printf 'a warning\n' >&2
exit 0
"#;

    /// Failing child: partial output, a diagnostic, exit 7.
    const FAKE_FAIL: &str = r#"#!/bin/sh
printf 'partial\n'
printf 'boom\n' >&2
exit 7
"#;

    /// A child that never answers: only the timeout can end it.
    const FAKE_SLOW: &str = r#"#!/bin/sh
printf 'started\n'
sleep 30
"#;

    /// More than `STREAM_CAP` bytes on stdout, with no newline at 64 KiB.
    const FAKE_BIG: &str = r#"#!/bin/sh
head -c 200000 /dev/zero | tr '\0' 'a'
"#;

    /// Delay of one `auth check` probe in the batch tests, in milliseconds. Small
    /// enough to keep the suite quick, long enough that a sequential run cannot
    /// fit inside the assertion's bound. Injected into the script below, so the
    /// body and the assertions can never drift apart.
    const AUTH_MANY_DELAY_MS: u64 = 300;

    /// Account probe: reads `--provider <id>` from its argv, appends a line to the
    /// marker for every run, and answers per id — `hang` never returns, `broken`
    /// answers garbage, everything else is a ready provider after the delay.
    const FAKE_AUTH_MANY: &str = r#"#!/bin/sh
printf 'ran\n' >> '@MARKER@'
prev=''
provider=''
for arg in "$@"; do
  if [ "$prev" = '--provider' ]; then provider="$arg"; fi
  prev="$arg"
done
case "$provider" in
  hang) sleep 30 ;;
  broken) printf 'boom\n'; exit 3 ;;
  *) sleep @DELAY@; printf '{"provider":"%s","status":"ready","credential":true}\n' "$provider" ;;
esac
"#;

    /// Interactive: greets, then echoes every stdin line back on stdout.
    const FAKE_ECHO: &str = r#"#!/bin/sh
printf 'ready line\n'
while IFS= read -r line; do
  printf 'echo:%s\n' "$line"
done
exit 0
"#;

    /// Interactive: streams two lines on two streams and exits on its own.
    const FAKE_STREAM: &str = r#"#!/bin/sh
printf 'line one\n'
printf 'warn one\n' >&2
printf 'line two\n'
exit 0
"#;

    /// Interactive: announces itself, ignores stdin and hangs, so only a kill
    /// can end it.
    const FAKE_HANG: &str = r#"#!/bin/sh
printf 'started\n'
printf 'started\n' >> '@MARKER@'
sleep 30
"#;

    /// Interactive login: refuses to run without a TTY (exactly what the real
    /// `pi auth login` does), then prints an ANSI-laden banner plus the URL,
    /// reads one pasted line and leaves cleanly.
    const FAKE_LOGIN: &str = r#"#!/bin/sh
if [ ! -t 0 ]; then
  printf 'requires an interactive terminal\n'
  exit 1
fi
printf '\033[1G\033[2K'
printf '\033[1mOpen this URL to sign in:\033[0m\n'
printf '\033[32mhttps://auth.example.test/oauth/authorize?client_id=abc&state=xyz\033[0m\n'
printf 'Callback: \033[36mhttp://127.0.0.1:1455/auth/callback\033[0m\n'
printf 'Paste the redirect URL here:\n'
IFS= read -r line
printf 'got:%s\n' "$line"
exit 0
"#;

    /// Writes a fake CLI script and makes it executable. `@MARKER@` becomes a
    /// path the script can touch so a test can prove it ran (or never did).
    #[cfg(unix)]
    fn write_script(scratch: &Scratch, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch.path(name);
        let marker = scratch.path("marker");
        std::fs::write(
            &path,
            body.replace("@MARKER@", &marker.display().to_string()),
        )
        .expect("write fake pi");
        let mut perms = std::fs::metadata(&path)
            .expect("stat fake pi")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod fake pi");
        path
    }

    #[cfg(unix)]
    fn binary(path: &Path) -> &str {
        path.to_str().expect("utf-8 path")
    }

    /// The account-probe script with its delay injected from
    /// `AUTH_MANY_DELAY_MS`, so the body and the wall-time bound share one number.
    #[cfg(unix)]
    fn auth_many_body() -> String {
        FAKE_AUTH_MANY.replace(
            "@DELAY@",
            &format!(
                "{}.{:03}",
                AUTH_MANY_DELAY_MS / 1000,
                AUTH_MANY_DELAY_MS % 1000
            ),
        )
    }

    /// Events collected from a session sink.
    #[derive(Default)]
    struct Collected {
        events: Mutex<Vec<CliEvent>>,
    }

    impl Collected {
        fn sink(self: &Arc<Self>) -> CliSink {
            let collected = Arc::clone(self);
            Arc::new(move |event: &CliEvent| {
                collected.events.lock().unwrap().push(event.clone());
            })
        }

        fn snapshot(&self) -> Vec<CliEvent> {
            self.events.lock().unwrap().clone()
        }

        /// Waits until `check` accepts the collected events, so no test races a
        /// reader thread. Panics with what it saw on timeout.
        fn wait_for(&self, what: &str, check: impl Fn(&[CliEvent]) -> bool) -> Vec<CliEvent> {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let events = self.snapshot();
                if check(&events) {
                    return events;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {what}: {events:#?}"
                );
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    fn lines_of(events: &[CliEvent], stream: CliStream) -> Vec<String> {
        events
            .iter()
            .filter(|event| event.stream == stream)
            .filter_map(|event| event.line.clone())
            .collect()
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    // ── non-interactive bridge (S1, S2) ─────────────────────────────────────

    // S2: the allowlist is the frozen contract's set, exactly. `mcp` joined it in
    // F0 of `tool-drawer-and-mcp.md`, because every MCP read and write is a
    // `pi mcp` call and nothing here edits `mcp.json`.
    #[test]
    fn allowlist_matches_the_frozen_contract() {
        assert_eq!(
            ALLOWED_SUBCOMMANDS,
            &["auth", "models", "config", "mcp", "--version", "--help"]
        );
    }

    // S1 of `tool-drawer-and-mcp.md`: `mcp` is reachable through the bridge, and
    // widening the allowlist did not let `install` (arbitrary third-party code)
    // or a prefix match through.
    #[cfg(unix)]
    #[test]
    fn allowlist_accepts_mcp_and_still_rejects_install() {
        let scratch = Scratch::new("run_mcp");
        let script = write_script(&scratch, "pi-ok.sh", FAKE_OK);

        let result = run(
            &args(&["mcp", "list", "--json"]),
            Duration::from_secs(10),
            binary(&script),
        )
        .expect("mcp is in the allowlist");
        assert_eq!(result.exit_code, Some(0));

        assert_eq!(
            run(
                &args(&["install", "some-package"]),
                Duration::from_secs(10),
                binary(&script),
            )
            .expect_err("install stays outside the allowlist"),
            CliError::NotAllowed("install".to_string())
        );
        // Nothing is matched by prefix: a longer word starting with an allowed
        // one is still refused.
        assert_eq!(
            run(
                &args(&["mcpx", "list"]),
                Duration::from_secs(10),
                binary(&script),
            )
            .expect_err("the allowlist is exact"),
            CliError::NotAllowed("mcpx".to_string())
        );
    }

    // S1: a normal call is captured whole, with its code.
    #[cfg(unix)]
    #[test]
    fn run_captures_stdout_and_a_zero_exit() {
        let scratch = Scratch::new("run_ok");
        let script = write_script(&scratch, "pi-ok.sh", FAKE_OK);

        let result = run(
            &args(&["--version"]),
            Duration::from_secs(10),
            binary(&script),
        )
        .expect("run an allowlisted command");

        assert_eq!(result.stdout, "hello from pi\n");
        assert_eq!(result.stderr, "a warning\n");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out, "the child left on its own: {result:?}");
        assert!(!result.truncated, "nothing was cut: {result:?}");
    }

    // S1: a failing child is an answer, not an error: the code and both streams
    // reach the caller.
    #[cfg(unix)]
    #[test]
    fn run_reports_a_non_zero_exit_code() {
        let scratch = Scratch::new("run_fail");
        let script = write_script(&scratch, "pi-fail.sh", FAKE_FAIL);

        let result = run(
            &args(&["auth", "check"]),
            Duration::from_secs(10),
            binary(&script),
        )
        .expect("a non-zero exit is still a result");

        assert_eq!(result.exit_code, Some(7));
        assert_eq!(result.stdout, "partial\n");
        assert_eq!(result.stderr, "boom\n");
        assert!(!result.timed_out);
    }

    // S2: the timeout is mandatory and it really kills the child.
    #[cfg(unix)]
    #[test]
    fn run_kills_the_child_when_the_timeout_expires() {
        let scratch = Scratch::new("run_timeout");
        let script = write_script(&scratch, "pi-slow.sh", FAKE_SLOW);

        let started = Instant::now();
        let result = run(
            &args(&["models"]),
            Duration::from_millis(300),
            binary(&script),
        )
        .expect("a timeout is a result, not an error");
        let elapsed = started.elapsed();

        assert!(
            result.timed_out,
            "the deadline must be reported: {result:?}"
        );
        assert_eq!(result.stdout, "started\n", "output before the kill is kept");
        assert!(
            elapsed < Duration::from_secs(5),
            "the injected deadline must bound the wait, took {elapsed:?}"
        );
        // The kill left nothing behind: the script's sleep cannot outlive it.
        // The process table can lag the kill for a moment under a loaded
        // machine, so poll briefly instead of asserting on a single snapshot.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut orphan = true;
        while Instant::now() < deadline {
            if !is_running(&script).0 {
                orphan = false;
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(!orphan, "a timed-out child must be killed, not orphaned");
    }

    // S2: output is bounded per stream and the cut is reported.
    #[cfg(unix)]
    #[test]
    fn run_caps_each_stream_at_64_kib_and_flags_truncation() {
        let scratch = Scratch::new("run_big");
        let script = write_script(&scratch, "pi-big.sh", FAKE_BIG);

        let result = run(&args(&["models"]), Duration::from_secs(30), binary(&script))
            .expect("run with output over the cap");

        assert_eq!(
            result.stdout.len(),
            STREAM_CAP,
            "stdout must stop at the cap, got {} bytes",
            result.stdout.len()
        );
        assert!(result.stdout.bytes().all(|byte| byte == b'a'));
        assert!(result.truncated, "overflow must be reported: {result:?}");
    }

    // S2: a subcommand outside the allowlist never reaches the spawn path.
    #[cfg(unix)]
    #[test]
    fn run_rejects_a_disallowed_subcommand_without_spawning() {
        let scratch = Scratch::new("run_denied");
        let script = write_script(&scratch, "pi-ok.sh", FAKE_OK);
        let marker = scratch.path("marker");

        let err = run(
            &args(&["install", "some-package"]),
            Duration::from_secs(10),
            binary(&script),
        )
        .expect_err("install is not in the allowlist");

        assert_eq!(err, CliError::NotAllowed("install".to_string()));
        assert!(
            !marker.exists(),
            "a rejected subcommand must never spawn the child"
        );
    }

    // S2: no arguments means no subcommand to check, so the call is refused
    // before any process exists.
    #[test]
    fn run_rejects_empty_arguments() {
        let err = run(&[], Duration::from_secs(1), "/nonexistent/pi")
            .expect_err("an empty argv has no allowlisted subcommand");
        assert_eq!(err, CliError::EmptyArgs);
        assert!(
            err.to_string().contains("auth"),
            "the message names the allowlist: {err}"
        );
    }

    // ── account state (S4) ──────────────────────────────────────────────────

    #[test]
    fn parse_auth_check_reads_the_json_status() {
        let status = parse_auth_check(r#"{"provider":"nan","status":"ready","credential":true}"#);
        assert_eq!(status.provider, "nan");
        assert_eq!(status.state, AuthState::Ready);
        assert!(status.credential);

        let not_ready = parse_auth_check(r#"{"provider":"nan","status":"not_ready"}"#);
        assert_eq!(not_ready.state, AuthState::NotReady);
        assert!(
            !not_ready.credential,
            "no credential field and not ready means no credential"
        );
    }

    // S4 contract shape: `{ provider, state, credential }`, snake_case state.
    #[test]
    fn pi_auth_status_serializes_with_the_contract_field_names() {
        let status = PiAuthStatus {
            provider: "nan".to_string(),
            state: AuthState::NotReady,
            credential: false,
        };
        assert_eq!(
            serde_json::to_value(&status).expect("serialize"),
            json!({ "provider": "nan", "state": "not_ready", "credential": false })
        );
    }

    // S4: unparsable input never panics and never invents a state.
    #[test]
    fn parse_auth_check_answers_unknown_for_unparsable_json() {
        for raw in [
            "",
            "not json",
            "[]",
            "null",
            r#"{"status":42}"#,
            r#"{"status":"nope"}"#,
        ] {
            let status = parse_auth_check(raw);
            assert_eq!(status.state, AuthState::Unknown, "raw: {raw:?}");
            assert!(!status.credential, "raw: {raw:?}");
        }
    }

    // S4: the documented exit codes are the fallback when the JSON has no status.
    #[test]
    fn auth_status_falls_back_to_the_documented_exit_codes() {
        for (code, expected) in [
            (0, AuthState::Ready),
            (1, AuthState::NotReady),
            (2, AuthState::Invalid),
            (9, AuthState::Unknown),
        ] {
            let status = auth_status("nan", r#"{"provider":"nan"}"#, Some(code));
            assert_eq!(status.state, expected, "exit code {code}");
        }
        // An explicit status always wins over the exit code.
        assert_eq!(
            auth_status("nan", r#"{"status":"not_ready"}"#, Some(0)).state,
            AuthState::NotReady
        );
        // Garbage stays unknown even next to a clean exit code, and the answer
        // still names the provider the caller asked about.
        let garbage = auth_status("nan", "boom", Some(0));
        assert_eq!(garbage.state, AuthState::Unknown);
        assert_eq!(garbage.provider, "nan");
    }

    // A single probe that outlived its deadline says nothing about the account.
    // Both the single and the batch path share this mapping, so they can never
    // disagree about what `timedOut` means.
    #[test]
    fn auth_status_from_result_keeps_the_probe_mapping() {
        let timed_out = PiCliResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            timed_out: true,
            truncated: false,
        };
        assert_eq!(
            auth_status_from_result("nan", &timed_out).state,
            AuthState::Unknown
        );

        let ready = PiCliResult {
            stdout: r#"{"provider":"nan","status":"ready"}"#.to_string(),
            stderr: String::new(),
            exit_code: Some(0),
            timed_out: false,
            truncated: false,
        };
        let status = auth_status_from_result("nan", &ready);
        assert_eq!(status.state, AuthState::Ready);
        assert_eq!(status.provider, "nan");

        let garbage = PiCliResult {
            stdout: "boom".to_string(),
            ..ready
        };
        assert_eq!(
            auth_status_from_result("nan", &garbage).state,
            AuthState::Unknown
        );
    }

    // ── many-provider auth probe (S2 of `providers-pane-refinements.md`) ─────

    // S2: every requested provider is reported, in the order the caller asked
    // for, so the batch is a drop-in replacement for the sequential loop.
    #[cfg(unix)]
    #[test]
    fn auth_check_many_reports_every_provider_in_the_requested_order() {
        let scratch = Scratch::new("auth_many_order");
        let script = write_script(&scratch, "pi-auth-many.sh", &auth_many_body());
        let requested = [
            "openai-codex",
            "anthropic",
            "google",
            "openai",
            "helmcode",
            "nan",
        ];
        let providers: Vec<String> = requested.iter().map(|name| name.to_string()).collect();

        let statuses = auth_check_many(&providers, Duration::from_secs(20), binary(&script));

        assert_eq!(statuses.len(), providers.len(), "one entry per provider");
        let reported: Vec<&str> = statuses.iter().map(|s| s.provider.as_str()).collect();
        assert_eq!(reported, requested, "the order must match the request");
        for status in &statuses {
            assert_eq!(status.state, AuthState::Ready, "{status:?}");
            assert!(status.credential, "{status:?}");
        }
    }

    // S2: the point of the command. Six probes of `AUTH_MANY_DELAY` each take
    // ~1.8 s one after another; a pool of four needs two batches. The bound is
    // deliberately loose (4x the parallel expectation, well under the sequential
    // sum) so a busy machine cannot make it flaky.
    #[cfg(unix)]
    #[test]
    fn auth_check_many_runs_the_probes_within_a_bounded_pool() {
        let scratch = Scratch::new("auth_many_pool");
        let script = write_script(&scratch, "pi-auth-many.sh", &auth_many_body());
        let providers: Vec<String> = (0..6).map(|index| format!("p{index}")).collect();
        let sequential = Duration::from_millis(AUTH_MANY_DELAY_MS * providers.len() as u64);

        let started = Instant::now();
        let statuses = auth_check_many(&providers, Duration::from_secs(20), binary(&script));
        let elapsed = started.elapsed();

        assert_eq!(statuses.len(), providers.len());
        assert!(statuses.iter().all(|s| s.state == AuthState::Ready));
        assert!(
            elapsed < Duration::from_millis(1200),
            "a pool of {AUTH_CHECK_POOL} must finish six {AUTH_MANY_DELAY_MS} ms probes in two \
             batches, not {sequential:?}; took {elapsed:?}"
        );
    }

    // S2: one provider that never answers must not abort the batch; it answers
    // `unknown` and every other provider keeps its real state.
    #[cfg(unix)]
    #[test]
    fn auth_check_many_marks_a_timed_out_provider_unknown_without_failing_the_rest() {
        let scratch = Scratch::new("auth_many_timeout");
        let script = write_script(&scratch, "pi-auth-many.sh", &auth_many_body());
        let providers = args(&["first", "hang", "last"]);

        let started = Instant::now();
        // The deadline is per provider and this one is global to the call: wide
        // enough for the fast probes (`AUTH_MANY_DELAY_MS`) and narrow enough to
        // cut the hanging one short.
        let statuses = auth_check_many(&providers, Duration::from_millis(1000), binary(&script));
        let elapsed = started.elapsed();

        assert_eq!(statuses.len(), 3);
        assert_eq!(statuses[0].state, AuthState::Ready, "{:?}", statuses[0]);
        assert_eq!(statuses[1].state, AuthState::Unknown, "{:?}", statuses[1]);
        assert_eq!(statuses[1].provider, "hang", "the entry keeps its provider");
        assert!(!statuses[1].credential, "{:?}", statuses[1]);
        assert_eq!(statuses[2].state, AuthState::Ready, "{:?}", statuses[2]);
        assert!(
            elapsed < Duration::from_secs(5),
            "the per-provider deadline must bound the batch, took {elapsed:?}"
        );
    }

    // S2: a provider whose answer is not usable is `unknown`, next to providers
    // that did answer.
    #[cfg(unix)]
    #[test]
    fn auth_check_many_marks_a_garbage_answer_unknown() {
        let scratch = Scratch::new("auth_many_garbage");
        let script = write_script(&scratch, "pi-auth-many.sh", &auth_many_body());
        let providers = args(&["broken", "fine"]);

        let statuses = auth_check_many(&providers, Duration::from_secs(20), binary(&script));

        assert_eq!(statuses[0].state, AuthState::Unknown, "{:?}", statuses[0]);
        assert_eq!(statuses[1].state, AuthState::Ready, "{:?}", statuses[1]);
    }

    // S2: no providers means no probes, and the answer is empty, not an error.
    #[cfg(unix)]
    #[test]
    fn auth_check_many_answers_empty_for_no_providers() {
        let scratch = Scratch::new("auth_many_empty");
        let script = write_script(&scratch, "pi-auth-many.sh", &auth_many_body());

        let statuses = auth_check_many(&[], Duration::from_secs(20), binary(&script));

        assert!(statuses.is_empty(), "{statuses:?}");
        assert!(
            !scratch.path("marker").exists(),
            "an empty request must not spawn anything"
        );
    }

    // S2: a blank id cannot name a provider, so it is reported `unknown` as-is
    // (the entry keeps its slot and its provider, so callers mapping by index or
    // by id both stay aligned) and it is never handed to the CLI.
    #[cfg(unix)]
    #[test]
    fn auth_check_many_answers_unknown_for_blank_provider_ids_without_spawning() {
        let scratch = Scratch::new("auth_many_blank");
        let script = write_script(&scratch, "pi-auth-many.sh", &auth_many_body());
        let providers = args(&["  ", "fine"]);

        let statuses = auth_check_many(&providers, Duration::from_secs(20), binary(&script));

        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].provider, "  ", "the slot keeps its input id");
        assert_eq!(statuses[0].state, AuthState::Unknown, "{:?}", statuses[0]);
        assert_eq!(statuses[1].state, AuthState::Ready, "{:?}", statuses[1]);
        let runs = std::fs::read_to_string(scratch.path("marker")).unwrap_or_default();
        assert_eq!(
            runs.lines().count(),
            1,
            "only the real provider may be spawned, marker: {runs:?}"
        );
    }

    // S2: a binary that cannot run is not a batch failure either: every entry is
    // `unknown`, which is exactly what the UI paints.
    #[test]
    fn auth_check_many_answers_unknown_when_the_probe_cannot_spawn() {
        let providers = args(&["openai", "anthropic"]);

        let statuses = auth_check_many(&providers, Duration::from_secs(20), "/nonexistent/pi");

        assert_eq!(statuses.len(), 2);
        for (status, provider) in statuses.iter().zip(providers.iter()) {
            assert_eq!(status.provider, *provider, "{status:?}");
            assert_eq!(status.state, AuthState::Unknown, "{status:?}");
            assert!(!status.credential, "{status:?}");
        }
    }

    // ── terminal text (S3) ─────────────────────────────────────────

    // S3: colors and SGR attributes never reach the webview.
    #[test]
    fn strip_ansi_removes_colors_and_attributes() {
        assert_eq!(
            strip_ansi("\x1b[1mSign in\x1b[0m with \x1b[38;5;208mChatGPT\x1b[39m"),
            "Sign in with ChatGPT"
        );
    }

    // S3: cursor, erase and screen sequences are dropped whole, including the
    // private-mode ones a login flow uses to hide the cursor.
    #[test]
    fn strip_ansi_removes_cursor_and_screen_sequences() {
        for raw in [
            "\x1b[1G\x1b[0Jopen",
            "\x1b[2J\x1b[Hopen",
            "\x1b[3A\x1b[Kopen",
            "\x1b[?25lopen\x1b[?25h",
            "\x1b[2;3Hopen",
        ] {
            assert_eq!(strip_ansi(raw), "open", "raw: {raw:?}");
        }
    }

    // S3: a carriage return overwrites the line it returns to, so only what came
    // after the last one is visible text.
    #[test]
    fn strip_ansi_keeps_only_what_survives_a_carriage_return() {
        assert_eq!(strip_ansi("loading 10%\rloading 40%\rDone"), "Done");
        assert_eq!(strip_ansi("discarded\rkept"), "kept");
        assert_eq!(strip_ansi("no carriage return"), "no carriage return");
        assert_eq!(
            strip_ansi("\x08\x08ok"),
            "ok",
            "a backspace erases one char"
        );
    }

    // S3: window titles arrive as OSC strings, BEL- or ST-terminated, and carry
    // no text worth showing.
    #[test]
    fn strip_ansi_drops_osc_sequences() {
        assert_eq!(
            strip_ansi("\x1b]0;pi \u{2014} login\x07Open this URL to sign in:"),
            "Open this URL to sign in:"
        );
        assert_eq!(
            strip_ansi("\x1b]8;;https://example.test\x1b\\after"),
            "after"
        );
    }

    // S3: the URL the human has to open must survive byte for byte.
    #[test]
    fn strip_ansi_leaves_a_url_untouched() {
        let url = "http://127.0.0.1:1455/auth/callback?code=abc123&state=xyz";
        assert_eq!(strip_ansi(&format!("\x1b[32m{url}\x1b[0m")), url);
    }

    // S3: pty output is split on `\r\n`, `\r` and `\n`, and a line cut by a
    // chunk boundary is rejoined instead of shown twice.
    #[test]
    fn line_splitter_splits_on_crlf_cr_and_lf() {
        let mut splitter = LineSplitter::default();
        assert_eq!(
            splitter.push(b"one\r\ntwo\rthree\nfour"),
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );
        assert_eq!(splitter.flush().as_deref(), Some("four"));
        assert_eq!(splitter.flush(), None, "the buffer is empty after a flush");
    }

    #[test]
    fn line_splitter_joins_chunks_that_split_a_line() {
        let mut splitter = LineSplitter::default();
        assert!(splitter.push(b"Open this URL").is_empty());
        assert_eq!(
            splitter.push(b" to sign in:\n"),
            vec!["Open this URL to sign in:".to_string()]
        );
        assert_eq!(splitter.flush(), None);
    }

    // S3: a line with nothing left after stripping is not a row in the webview.
    #[test]
    fn line_splitter_drops_lines_that_are_empty_after_stripping() {
        let mut splitter = LineSplitter::default();
        assert!(splitter.push(b"\n\x1b[2K\n   \n").is_empty());
    }

    // ── interactive bridge (S1, S3) ─────────────────────────────────────────

    // S3: the pty merges both streams into the one line stream a terminal has,
    // in write order, and the exit is reported.
    #[cfg(unix)]
    #[test]
    fn start_streams_lines_and_the_exit_event() {
        let scratch = Scratch::new("start_stream");
        let script = write_script(&scratch, "pi-stream.sh", FAKE_STREAM);
        let sessions = CliSessions::new();
        let collected = Arc::new(Collected::default());

        let id = sessions
            .start(&args(&["auth"]), binary(&script), collected.sink())
            .expect("start an allowlisted command");
        assert!(
            id.starts_with("cli-"),
            "the id is a session handle, got {id}"
        );

        let events = collected.wait_for("the exit event", |events| {
            events.iter().any(|event| event.stream == CliStream::Exit)
        });

        assert_eq!(
            lines_of(&events, CliStream::Stdout),
            vec![
                "line one".to_string(),
                "warn one".to_string(),
                "line two".to_string()
            ],
            "a terminal merges stderr into stdout, whole and in order: {events:#?}"
        );
        assert!(
            lines_of(&events, CliStream::Stderr).is_empty(),
            "the pty has one stream, so nothing is reported as stderr: {events:#?}"
        );
        let exit = events
            .iter()
            .find(|event| event.stream == CliStream::Exit)
            .expect("exit event");
        assert_eq!(exit.exit_code, Some(0));
        assert_eq!(exit.line, None, "an exit carries a code, not a line");
        assert!(
            events.iter().all(|event| event.cli_session_id == id),
            "every event names its session: {events:#?}"
        );
        // The registry tracks the id until the host stops it, so a late `stop`
        // is what reaps the finished child.
        assert_eq!(sessions.live_ids(), vec![id.clone()]);
        assert!(sessions.stop(&id).expect("stop after a natural exit"));
        assert!(
            sessions.live_ids().is_empty(),
            "a stopped session is no longer live"
        );
    }

    // S3 (defect L6 of `pi-cli-bridge.md`): the interactive session runs on a
    // pty, so a child that demands a terminal starts; the ANSI is stripped, the
    // URL survives byte for byte, a pasted redirect URL reaches the child as one
    // line and the child's own exit code is reported.
    #[cfg(unix)]
    #[test]
    fn pty_session_emits_clean_lines_and_receives_the_pasted_url() {
        let scratch = Scratch::new("pty_login");
        let script = write_script(&scratch, "pi-login.sh", FAKE_LOGIN);
        let sessions = CliSessions::new();
        let collected = Arc::new(Collected::default());

        let id = sessions
            .start(
                &args(&["auth", "login", "openai", "--no-browser"]),
                binary(&script),
                collected.sink(),
            )
            .expect("start the login flow");

        let events = collected.wait_for("the paste prompt", |events| {
            lines_of(events, CliStream::Stdout)
                .iter()
                .any(|line| line == "Paste the redirect URL here:")
        });
        let lines = lines_of(&events, CliStream::Stdout);
        assert!(
            lines.iter().all(|line| !line.contains('\x1b')),
            "no escape character may reach the webview: {lines:#?}"
        );
        assert!(
            lines.iter().all(|line| !line.contains('\x07')),
            "no bell from an OSC title may reach the webview: {lines:#?}"
        );
        assert!(
            lines.contains(&"Open this URL to sign in:".to_string()),
            "the prompt is intact: {lines:#?}"
        );
        assert!(
            lines.contains(
                &"https://auth.example.test/oauth/authorize?client_id=abc&state=xyz".to_string()
            ),
            "the URL survives byte for byte: {lines:#?}"
        );
        assert!(
            lines.contains(&"Callback: http://127.0.0.1:1455/auth/callback".to_string()),
            "the callback line is intact: {lines:#?}"
        );
        assert!(
            lines.iter().all(|line| !line.trim().is_empty()),
            "empty lines are dropped instead of becoming rows: {lines:#?}"
        );

        let pasted = "http://127.0.0.1:1455/auth/callback?code=abc123&state=xyz";
        assert!(
            sessions
                .write(&id, &format!("{pasted}\r\n"))
                .expect("paste the redirect URL"),
            "a live pty session accepts input"
        );

        let events = collected.wait_for("the child's answer and exit", |events| {
            events.iter().any(|event| event.stream == CliStream::Exit)
        });
        let lines = lines_of(&events, CliStream::Stdout);
        assert!(
            lines.contains(&format!("got:{pasted}")),
            "the pasted URL arrives as a single line: {lines:#?}"
        );
        assert!(
            !lines.iter().any(|line| line == "got:"),
            "a pasted line ending must not become an empty answer: {lines:#?}"
        );
        let exit = events
            .iter()
            .find(|event| event.stream == CliStream::Exit)
            .expect("exit event");
        assert_eq!(
            exit.exit_code,
            Some(0),
            "the child's own exit code is reported: {events:#?}"
        );

        assert!(sessions.stop(&id).expect("stop after a natural exit"));
    }

    // S3: a pasted device code reaches the child's stdin, newline included.
    #[cfg(unix)]
    #[test]
    fn write_reaches_the_child_stdin() {
        let scratch = Scratch::new("write");
        let script = write_script(&scratch, "pi-echo.sh", FAKE_ECHO);
        let sessions = CliSessions::new();
        let collected = Arc::new(Collected::default());
        let id = sessions
            .start(
                &args(&["auth", "login", "nan", "--no-browser"]),
                binary(&script),
                collected.sink(),
            )
            .expect("start the login flow");

        // The greeting proves the child is up and reading before we write.
        collected.wait_for("the greeting", |events| {
            lines_of(events, CliStream::Stdout)
                .iter()
                .any(|line| line == "ready line")
        });

        assert!(
            sessions.write(&id, "device-code-123").expect("write"),
            "a live session accepts input"
        );

        // The child only echoes complete lines, so the newline was appended.
        collected.wait_for("the echoed code", |events| {
            lines_of(events, CliStream::Stdout)
                .iter()
                .any(|line| line == "echo:device-code-123")
        });

        sessions.stop(&id).expect("stop the login flow");
    }

    // S3: stop kills a hanging child and removes the session, idempotently.
    #[cfg(unix)]
    #[test]
    fn stop_kills_the_child_and_removes_the_session() {
        let scratch = Scratch::new("stop");
        let script = write_script(&scratch, "pi-hang.sh", FAKE_HANG);
        let marker = scratch.path("marker");
        let sessions = CliSessions::new();
        let collected = Arc::new(Collected::default());
        let id = sessions
            .start(&args(&["auth", "login"]), binary(&script), collected.sink())
            .expect("start a hanging child");

        collected.wait_for("the child's first line", |events| {
            lines_of(events, CliStream::Stdout)
                .iter()
                .any(|line| line == "started")
        });
        // The line and the marker are two separate writes in the fake child: it
        // prints, then appends. Waiting for the line and asserting the marker
        // immediately is an ordering race the whole suite can win by being busy —
        // it flaked exactly once, under load. Poll the marker instead of assuming
        // the second write already happened.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(marker.exists(), "the fake child must have run at all");

        let started = Instant::now();
        assert!(
            sessions.stop(&id).expect("stop"),
            "a live session is stopped"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "stop must not wait out the child, took {:?}",
            started.elapsed()
        );
        assert!(
            !sessions.stop(&id).expect("second stop"),
            "the session is removed, so stop is idempotent"
        );
        assert!(
            !sessions.write(&id, "late").expect("write after stop"),
            "an unknown session id answers false"
        );
        // The exit event proves the kill actually landed.
        collected.wait_for("the exit event", |events| {
            events.iter().any(|event| event.stream == CliStream::Exit)
        });
    }

    #[test]
    fn write_and_stop_answer_false_for_an_unknown_session() {
        let sessions = CliSessions::new();
        assert!(!sessions.write("cli-unknown", "text").expect("write"));
        assert!(!sessions.stop("cli-unknown").expect("stop"));
    }

    // S2: the interactive path enforces the same allowlist, without spawning.
    #[cfg(unix)]
    #[test]
    fn start_rejects_a_disallowed_subcommand_without_spawning() {
        let scratch = Scratch::new("start_denied");
        let script = write_script(&scratch, "pi-stream.sh", FAKE_STREAM);
        let marker = scratch.path("marker");
        let sessions = CliSessions::new();
        let collected = Arc::new(Collected::default());

        let err = sessions
            .start(&args(&["update"]), binary(&script), collected.sink())
            .expect_err("update is not in the allowlist");

        assert_eq!(err, CliError::NotAllowed("update".to_string()));
        assert!(!marker.exists(), "nothing may be spawned for a denied call");
        assert!(
            collected.snapshot().is_empty(),
            "a denied call emits no session event"
        );
    }

    /// True when a process still runs the given script path. Used to prove a
    /// killed child left nothing behind.
    #[cfg(unix)]
    fn is_running(script: &Path) -> (bool, String) {
        let name = script
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let output = Command::new("ps")
            .args(["-A", "-o", "command="])
            .output()
            .expect("ps");
        let listing = String::from_utf8_lossy(&output.stdout).to_string();
        (listing.lines().any(|line| line.contains(name)), listing)
    }
}
