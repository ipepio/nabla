// F2 of `tool-drawer-and-mcp.md` (S2, S5): the workspace terminal.
//
// One pty per tab, spawned on the user's own shell with `cwd` at the workspace,
// streaming raw bytes to the webview so xterm renders them. The module is the
// whole lifecycle — open, write, resize, close — because a pty that loses one of
// those handles becomes a process nobody can address.
//
// Frozen contract: `ShellSession` is what `shell_open` answers, `ShellOutputEvent` is
// the `shell-output` payload, both camelCase so the TS contracts mirror them
// field by field. The terminal is **not** exposed to the agent: pi already has
// its own shell tool, and a second one here would only widen the surface.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtyPair, PtySize, native_pty_system};

/// How many terminals may be live at once. A tab per workspace is the intended
/// use; the cap is there so a stuck UI cannot leak processes without bound.
pub const MAX_SESSIONS: usize = 8;

/// Initial geometry, until xterm reports the real size of the panel. 80x24 is
/// what a terminal assumes by default, so a shell paints its prompt normally
/// before the first resize.
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

/// What the child is told it is rendering into. xterm speaks this one.
pub const TERM: &str = "xterm-256color";

/// Grace given to a child to leave after the polite terminate signal before the
/// force kill. Same bound as the CLI bridge, so both kill paths feel alike.
const KILL_GRACE: Duration = Duration::from_millis(500);

/// Bytes read from the pty at once: one event per read, no line splitting (a
/// terminal renders partial lines and escape sequences as they arrive).
const CHUNK: usize = 8192;

/// Ring-buffer cap per terminal so MCP `terminal_read` can inspect recent output
/// without unbounded memory growth.
const OUTPUT_RING_CAP: usize = 65_536;

/// Snapshot of one live or recently-exited terminal for MCP discovery.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveShell {
    pub session_id: String,
    pub pid: Option<u32>,
    pub shell: String,
    pub cwd: String,
    pub alive: bool,
}

/// The environment a shell is allowed to see. Everything else — provider keys,
/// tokens, git identities — stays out: the terminal belongs to the human, and
/// forwarding the runtime's allowlist would hand the child more than it needs.
const FORWARDED_ENV: &[&str] = &["PATH", "HOME", "LANG"];

/// Handle of one terminal, as handed to the webview.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellSession {
    pub session_id: String,
    /// Session leader of the pty. Serialized as `null` when the platform does not
    /// report one, so the TS contract's `pid: number | null` is always there.
    pub pid: Option<u32>,
    /// The program that was spawned, so the tab can show what it runs.
    pub shell: String,
}

/// The two things one terminal reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellOutputStream {
    /// Raw pty bytes, already decoded for xterm to render.
    Data,
    /// The child is gone and its exit code is known, if it had one.
    Exit,
}

/// Payload of the Tauri `shell-output` event:
/// `{ sessionId, stream, data?, exitCode? }`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellOutputEvent {
    pub session_id: String,
    pub stream: ShellOutputStream,
    /// Present on `data`: what the child wrote, lossily decoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// Present on `exit`, and only when the child left on its own rather than
    /// being killed by a signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// Where every terminal event goes. The command layer turns this into the Tauri
/// `shell-output` event; tests collect it in a vector.
pub type ShellSink = Arc<dyn Fn(&ShellOutputEvent) + Send + Sync>;

/// Failures of the terminal. Plain strings cross the Tauri bridge; this enum
/// keeps them typed and testable inside Rust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellError {
    /// `MAX_SESSIONS` terminals are already live.
    TooMany(usize),
    /// The child could not be started at all.
    Spawn(String),
    /// Writing to a live child's terminal failed.
    Write(String),
    /// Telling the kernel about a new window size failed.
    Resize(String),
}

impl std::fmt::Display for ShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellError::TooMany(cap) => write!(
                f,
                "terminal: {cap} sessions are already open; close one before opening another"
            ),
            ShellError::Spawn(message) => write!(f, "terminal: {message}"),
            ShellError::Write(message) => write!(f, "terminal: write failed: {message}"),
            ShellError::Resize(message) => write!(f, "terminal: resize failed: {message}"),
        }
    }
}

impl std::error::Error for ShellError {}

/// The shell to spawn (S2): `$SHELL` when it names something real, then
/// `/bin/zsh`, then `/bin/bash`.
pub fn resolve_shell() -> String {
    resolve_shell_with(std::env::var("SHELL").ok(), |path| path.is_file())
}

/// The resolution rule, injectable so it can be tested without depending on what
/// this machine happens to have installed. A candidate that does not exist is
/// skipped — a `$SHELL` left over from an uninstalled shell would otherwise open
/// a tab that can never start. When no candidate exists the last one is still
/// named, so the spawn reports its own clear failure instead of an empty argv.
fn resolve_shell_with(configured: Option<String>, exists: impl Fn(&Path) -> bool) -> String {
    let configured = configured.filter(|shell| !shell.trim().is_empty());
    let candidates: Vec<String> = configured
        .into_iter()
        .chain(
            ["/bin/zsh", "/bin/bash"]
                .iter()
                .map(|shell| shell.to_string()),
        )
        .collect();
    candidates
        .iter()
        .find(|shell| exists(Path::new(shell)))
        .cloned()
        .unwrap_or_else(|| {
            candidates
                .last()
                .cloned()
                .unwrap_or_else(|| "/bin/bash".to_string())
        })
}

/// Registry of live terminals. Owned by `AppState` and shared with the MCP driver.
#[derive(Default, Clone)]
pub struct ShellSessions {
    sessions: Arc<Mutex<HashMap<String, Arc<ShellHandle>>>>,
    next_id: Arc<AtomicUsize>,
}

impl ShellSessions {
    pub fn new() -> ShellSessions {
        ShellSessions::default()
    }

    /// Spawns `command` on a fresh pty of the given size, with `cwd` as its
    /// working directory, and streams its output to `sink`.
    ///
    /// The session is registered before the reader thread starts, so the first
    /// event can never arrive for an id nobody knows yet. The registry lock is
    /// held across the spawn: two concurrent opens must not both see the last
    /// free slot, and a fork is not I/O, so no writer waits on anything slow.
    pub fn open_with(
        &self,
        command: &str,
        args: &[String],
        cwd: &Path,
        cols: u16,
        rows: u16,
        sink: ShellSink,
    ) -> Result<ShellSession, ShellError> {
        let mut sessions = lock(&self.sessions);
        if sessions.len() >= MAX_SESSIONS {
            return Err(ShellError::TooMany(MAX_SESSIONS));
        }
        let PtyPair { slave, master } = native_pty_system()
            .openpty(geometry(cols, rows))
            .map_err(|err| ShellError::Spawn(format!("failed to open a pty: {err}")))?;
        let mut child = slave
            .spawn_command(shell_command(command, args, cwd))
            .map_err(|err| ShellError::Spawn(format!("failed to spawn {command}: {err}")))?;
        // The slave end must not stay open here: a reader only sees the terminal
        // hang up once the last slave descriptor is gone.
        drop(slave);

        let pid = child.process_id();
        let reader = master
            .try_clone_reader()
            .map_err(|err| ShellError::Spawn(format!("failed to read the pty: {err}")))?;
        let writer = master
            .take_writer()
            .map_err(|err| ShellError::Spawn(format!("failed to write to the pty: {err}")))?;
        let killer = child.clone_killer();

        let session_id = self.next_session_id();
        let exited = Arc::new(AtomicBool::new(false));
        let output_buf = Arc::new(Mutex::new(VecDeque::with_capacity(CHUNK)));
        sessions.insert(
            session_id.clone(),
            Arc::new(ShellHandle {
                pid,
                shell: command.to_string(),
                cwd: cwd.display().to_string(),
                // Kept alive and kept addressable: dropping the master closes the
                // terminal, and `resize` needs it.
                master: Mutex::new(master),
                writer: Mutex::new(Some(writer)),
                killer: Mutex::new(Some(killer)),
                exited: Arc::clone(&exited),
                output_buf: Arc::clone(&output_buf),
            }),
        );
        drop(sessions);

        let reader_done = Arc::new(AtomicBool::new(false));
        stream_pty(
            reader,
            session_id.clone(),
            Arc::clone(&sink),
            Arc::clone(&output_buf),
            Arc::clone(&reader_done),
        );

        // Exit watcher: it owns the `Child`, so it reaps the process without
        // holding any lock the writer needs.
        let exit_id = session_id.clone();
        thread::spawn(move || {
            let code = child.wait().ok().as_ref().and_then(exit_code_of);
            exited.store(true, Ordering::SeqCst);
            // The reader goes first: a `data` event after `exit` would reach a tab
            // that already painted "session over". Bounded, because a descendant
            // may hold the terminal open after its leader is gone.
            let _ = wait_flag(&reader_done, KILL_GRACE);
            sink(&ShellOutputEvent {
                session_id: exit_id,
                stream: ShellOutputStream::Exit,
                data: None,
                exit_code: code,
            });
        });

        Ok(ShellSession {
            session_id,
            pid,
            shell: command.to_string(),
        })
    }

    /// Writes what the human typed, byte for byte. No newline is appended: xterm
    /// sends the carriage return itself, and adding one would turn a keystroke
    /// into a submitted line. `Ok(false)` for an unknown or finished session, so
    /// a late write is not an error.
    pub fn write(&self, session_id: &str, data: &str) -> Result<bool, ShellError> {
        let Some(handle) = self.handle(session_id) else {
            return Ok(false);
        };
        // A child that already exited has nothing left to read.
        if handle.exited.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let mut guard = lock(&handle.writer);
        let Some(terminal) = guard.as_mut() else {
            return Ok(false);
        };
        terminal
            .write_all(data.as_bytes())
            .and_then(|()| terminal.flush())
            .map_err(|err| ShellError::Write(err.to_string()))?;
        Ok(true)
    }

    /// Tells the kernel (and through it, the child) the new panel size, so the
    /// shell reflows and full-screen programs like `vim` get their signals.
    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<bool, ShellError> {
        let Some(handle) = self.handle(session_id) else {
            return Ok(false);
        };
        lock(&handle.master)
            .resize(geometry(cols, rows))
            .map_err(|err| ShellError::Resize(err.to_string()))?;
        Ok(true)
    }

    /// Kills the session's child if it is still alive and removes the session.
    /// `false` for an unknown id, so closing a tab twice is harmless.
    pub fn close(&self, session_id: &str) -> bool {
        let Some(handle) = lock(&self.sessions).remove(session_id) else {
            return false;
        };
        // Close the terminal first: a shell that reads it sees EOF and may leave
        // on its own before any signal is needed.
        lock(&handle.writer).take();
        if handle.exited.load(Ordering::SeqCst) {
            return true;
        }
        terminate(&handle);
        if !wait_flag(&handle.exited, KILL_GRACE) {
            // A child that ignores the polite signal must not outlive the window.
            force_kill(&handle);
            let _ = wait_flag(&handle.exited, KILL_GRACE);
        }
        true
    }

    /// Lists all open terminal sessions in stable `session_id` order.
    pub fn list(&self) -> Vec<LiveShell> {
        let mut out: Vec<LiveShell> = lock(&self.sessions)
            .iter()
            .map(|(id, handle)| LiveShell {
                session_id: id.clone(),
                pid: handle.pid,
                shell: handle.shell.clone(),
                cwd: handle.cwd.clone(),
                alive: !handle.exited.load(Ordering::SeqCst),
            })
            .collect();
        out.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        out
    }

    /// Reads the most recent lines from a terminal's ring buffer, optionally
    /// stripping ANSI escape sequences for clean LLM consumption.
    pub fn read_output(&self, session_id: &str, max_lines: usize, strip_ansi: bool) -> Option<String> {
        let handle = self.handle(session_id)?;
        let raw_bytes: Vec<u8> = {
            let guard = lock(&handle.output_buf);
            guard.iter().copied().collect()
        };
        let decoded = String::from_utf8_lossy(&raw_bytes);
        let text = if strip_ansi {
            strip_ansi_codes(&decoded)
        } else {
            decoded.into_owned()
        };
        let limit = max_lines.clamp(1, 500);
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.len().saturating_sub(limit);
        Some(lines[start..].join("\n"))
    }

    /// Closes every live session (S2: no terminal outlives the host).
    pub fn close_all(&self) {
        let ids: Vec<String> = lock(&self.sessions).keys().cloned().collect();
        for id in ids {
            self.close(&id);
        }
    }

    fn handle(&self, session_id: &str) -> Option<Arc<ShellHandle>> {
        lock(&self.sessions).get(session_id).cloned()
    }

    fn next_session_id(&self) -> String {
        let sequence = self.next_id.fetch_add(1, Ordering::SeqCst);
        format!("shell-{}-{sequence}", std::process::id())
    }
}

impl Drop for ShellSessions {
    fn drop(&mut self) {
        if Arc::strong_count(&self.sessions) == 1 {
            self.close_all();
        }
    }
}

/// One managed terminal and the handles its threads need.
struct ShellHandle {
    /// Session leader of the pty, when the platform reports one.
    pid: Option<u32>,
    /// Spawned shell executable path/name.
    shell: String,
    /// Working directory the terminal started in.
    cwd: String,
    /// The pty itself. Dropping it closes the terminal.
    master: Mutex<Box<dyn MasterPty + Send>>,
    /// Taken by `close` to hang up; a live session always has it.
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    /// The pty's own kill path, used only when no pid is available.
    killer: Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>,
    /// Set by the exit watcher once the child is reaped.
    exited: Arc<AtomicBool>,
    /// Recent PTY output ring buffer (bounded to `OUTPUT_RING_CAP` bytes).
    output_buf: Arc<Mutex<VecDeque<u8>>>,
}

/// Strips ANSI CSI/OSC escape sequences and carriage returns so an agent reading
/// a terminal buffer gets clean human-readable lines.
pub fn strip_ansi_codes(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            i += 1;
            if i >= bytes.len() {
                break;
            }
            match bytes[i] {
                // CSI: ESC [ ... final byte 0x40..=0x7E
                b'[' => {
                    i += 1;
                    while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1;
                    }
                }
                // OSC: ESC ] ... BEL (0x07) or ST (ESC \)
                b']' => {
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                // Two-byte escape sequence
                _ => {
                    i += 1;
                }
            }
        } else if bytes[i] == b'\r' {
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Forwards pty output as `data` events until the terminal closes (S2).
///
/// The bytes are decoded lossily: a read can end inside a multi-byte character,
/// and buffering the tail would delay the escape sequences after it. xterm shows
/// the resulting replacement glyph once; the next chunk is intact again.
fn stream_pty(
    reader: Box<dyn Read + Send>,
    session_id: String,
    sink: ShellSink,
    output_buf: Arc<Mutex<VecDeque<u8>>>,
    done: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let mut reader = reader;
        let mut chunk = [0u8; CHUNK];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(filled) => {
                    {
                        let mut buf = lock(&output_buf);
                        let slice = &chunk[..filled];
                        if slice.len() >= OUTPUT_RING_CAP {
                            buf.clear();
                            buf.extend(&slice[slice.len() - OUTPUT_RING_CAP..]);
                        } else {
                            let overflow = (buf.len() + slice.len()).saturating_sub(OUTPUT_RING_CAP);
                            if overflow > 0 {
                                buf.drain(..overflow);
                            }
                            buf.extend(slice);
                        }
                    }
                    sink(&ShellOutputEvent {
                        session_id: session_id.clone(),
                        stream: ShellOutputStream::Data,
                        data: Some(String::from_utf8_lossy(&chunk[..filled]).into_owned()),
                        exit_code: None,
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                // A pty whose child is gone reports EIO on some platforms and a
                // clean end of file on others: both mean the terminal is over.
                Err(_) => break,
            }
        }
        done.store(true, Ordering::SeqCst);
    });
}

/// Builds the child of a terminal. The pty supplies its stdio on both platforms,
/// so only the argv, the directory and the environment are set here.
fn shell_command(command: &str, args: &[String], cwd: &Path) -> CommandBuilder {
    let mut builder = CommandBuilder::new(command);
    builder.args(args);
    builder.cwd(cwd);
    builder.env_clear();
    for name in FORWARDED_ENV {
        if let Ok(value) = std::env::var(name) {
            builder.env(name, value);
        }
    }
    builder.env("TERM", TERM);
    builder
}

/// The pty size. A zero would come from a panel that has not been laid out yet
/// and makes a shell print one character per line, so both axes have a floor.
fn geometry(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Exit code of a pty child, or `None` when a signal ended it (no code exists).
fn exit_code_of(status: &portable_pty::ExitStatus) -> Option<i32> {
    match status.signal() {
        Some(_) => None,
        None => Some(status.exit_code() as i32),
    }
}

/// Polite termination of one terminal: the shared process-group signal when the
/// pty named a session leader (so the shell's descendants go too), the pty handle
/// otherwise.
fn terminate(handle: &ShellHandle) {
    match handle.pid {
        Some(pid) => crate::process::signal_terminate(pid),
        None => kill_with_handle(handle),
    }
}

/// Last resort for a child that ignores the polite signal.
fn force_kill(handle: &ShellHandle) {
    match handle.pid {
        Some(pid) => crate::process::signal_kill(pid),
        None => kill_with_handle(handle),
    }
}

/// Kills through the pty's own handle, which is the only option when the platform
/// did not report a pid.
fn kill_with_handle(handle: &ShellHandle) {
    let mut guard = lock(&handle.killer);
    if let Some(killer) = guard.as_mut() {
        let _ = killer.kill();
    }
}

/// Waits for a latch within `budget`. Polled, never spun: the interval is a sleep
/// and the budget bounds the whole wait.
fn wait_flag(flag: &AtomicBool, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if flag.load(Ordering::SeqCst) {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    flag.load(Ordering::SeqCst)
}

/// Locks without panicking: a poisoned lock in a background thread must not turn
/// into a terminal the host can no longer address.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;
    use std::thread;
    use std::time::{Duration, Instant};

    /// Scratch directory in the system temp dir (never the worktree), removed on
    /// drop.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let mut dir = std::env::temp_dir();
            dir.push(format!(
                "nabla_shell_{}_{}_{tag}",
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

    // ── fake shells: no real interactive shell is ever spawned ──────────────

    /// Echoes a line, waits for one line of input, echoes it back and exits 3.
    const FAKE_SHELL: &str = r#"#!/bin/sh
printf 'ready line\n'
IFS= read -r line
printf 'echo:%s\n' "$line"
exit 3
"#;

    /// Announces itself and then never answers, so only a close can end it.
    const FAKE_IDLE: &str = r#"#!/bin/sh
printf 'idle\n'
while IFS= read -r line; do :; done
exit 0
"#;

    #[cfg(unix)]
    fn write_script(scratch: &Scratch, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch.path(name);
        std::fs::write(&path, body).expect("write fake shell");
        let mut perms = std::fs::metadata(&path)
            .expect("stat fake shell")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod fake shell");
        path
    }

    #[cfg(unix)]
    fn binary(path: &Path) -> &str {
        path.to_str().expect("utf-8 path")
    }

    /// Events collected from a session sink.
    #[derive(Default)]
    struct Collected {
        events: StdMutex<Vec<ShellOutputEvent>>,
    }

    impl Collected {
        fn sink(self: &Arc<Self>) -> ShellSink {
            let collected = Arc::clone(self);
            Arc::new(move |event: &ShellOutputEvent| {
                collected.events.lock().unwrap().push(event.clone());
            })
        }

        fn snapshot(&self) -> Vec<ShellOutputEvent> {
            self.events.lock().unwrap().clone()
        }

        /// Waits until `check` accepts the collected events, so no test races a
        /// reader thread. Panics with what it saw on timeout.
        fn wait_for(
            &self,
            what: &str,
            check: impl Fn(&[ShellOutputEvent]) -> bool,
        ) -> Vec<ShellOutputEvent> {
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

    /// Everything the child wrote, as one string.
    fn data_of(events: &[ShellOutputEvent]) -> String {
        events
            .iter()
            .filter(|event| event.stream == ShellOutputStream::Data)
            .filter_map(|event| event.data.clone())
            .collect()
    }

    fn contains(needle: &str) -> impl Fn(&[ShellOutputEvent]) -> bool + '_ {
        move |events| data_of(events).contains(needle)
    }

    fn exited(events: &[ShellOutputEvent]) -> Option<i32> {
        events
            .iter()
            .find(|event| event.stream == ShellOutputStream::Exit)
            .and_then(|event| event.exit_code)
    }

    // ── the pty lifecycle (S2) ─────────────────────────────────────────

    // S2: the pty streams what the child writes, as lossy UTF-8 the UI can feed
    // to xterm directly.
    #[cfg(unix)]
    #[test]
    fn open_with_streams_the_child_output_through_the_sink() {
        let scratch = Scratch::new("stream");
        let script = write_script(&scratch, "fake-shell.sh", FAKE_SHELL);
        let collected = Arc::new(Collected::default());
        let sessions = ShellSessions::new();

        let session = sessions
            .open_with(binary(&script), &[], &scratch.dir, 80, 24, collected.sink())
            .expect("open a pty");

        assert!(!session.session_id.is_empty(), "a session has an id");
        assert_eq!(session.shell, binary(&script));

        let events = collected.wait_for("the greeting", contains("ready line"));
        assert!(
            events
                .iter()
                .all(|event| event.session_id == session.session_id),
            "every event carries the id its session was handed out with: {events:#?}"
        );
        sessions.close(&session.session_id);
    }

    // S2: what the human types reaches the child's stdin, which is what makes
    // the tab a terminal instead of a log view.
    #[cfg(unix)]
    #[test]
    fn open_with_delivers_written_input_to_the_child() {
        let scratch = Scratch::new("write");
        let script = write_script(&scratch, "fake-shell.sh", FAKE_SHELL);
        let collected = Arc::new(Collected::default());
        let sessions = ShellSessions::new();

        let session = sessions
            .open_with(binary(&script), &[], &scratch.dir, 80, 24, collected.sink())
            .expect("open a pty");
        collected.wait_for("the greeting", contains("ready line"));

        assert!(
            sessions
                .write(&session.session_id, "hello\n")
                .expect("write"),
            "a live session accepts input"
        );
        collected.wait_for("the echoed line", contains("echo:hello"));
        sessions.close(&session.session_id);
    }

    // S2: the tab learns how the shell ended, so it can show "session over"
    // instead of a frozen cursor. The exit payload carries the code and no data.
    #[cfg(unix)]
    #[test]
    fn open_with_reports_the_exit_code_of_the_child() {
        let scratch = Scratch::new("exit");
        let script = write_script(&scratch, "fake-shell.sh", FAKE_SHELL);
        let collected = Arc::new(Collected::default());
        let sessions = ShellSessions::new();

        let session = sessions
            .open_with(binary(&script), &[], &scratch.dir, 80, 24, collected.sink())
            .expect("open a pty");
        collected.wait_for("the greeting", contains("ready line"));
        sessions.write(&session.session_id, "bye\n").expect("write");

        let events = collected.wait_for("the exit event", |events| exited(events).is_some());
        assert_eq!(exited(&events), Some(3), "the script exits 3");
        let exit = events
            .iter()
            .find(|event| event.stream == ShellOutputStream::Exit)
            .expect("an exit event");
        assert_eq!(exit.data, None, "an exit event carries no data");
        assert!(
            participants_agree(&events),
            "exit arrives once, after the last data event"
        );
        // A finished shell is still closable, and closing it is not an error.
        assert!(sessions.close(&session.session_id));
    }

    /// The exit event is last and unique: a tab can trust it to stop rendering.
    fn participants_agree(events: &[ShellOutputEvent]) -> bool {
        let exits = events
            .iter()
            .filter(|event| event.stream == ShellOutputStream::Exit)
            .count();
        exits == 1
            && events
                .last()
                .map(|event| event.stream == ShellOutputStream::Exit)
                .unwrap_or(false)
    }

    // S2: closing an id nobody handed out is a no-op, not an error — a tab can
    // close twice without special-casing it.
    #[test]
    fn close_an_unknown_session_answers_false() {
        let sessions = ShellSessions::new();
        assert!(!sessions.close("shell-does-not-exist"));
        assert!(!sessions.write("shell-does-not-exist", "x").expect("write"));
        assert!(
            !sessions
                .resize("shell-does-not-exist", 100, 30)
                .expect("resize")
        );
    }

    // S2: the cap is a live count, not a lifetime quota. Eight are allowed, the
    // ninth is refused with a message the UI can show, and closing one frees the
    // slot again.
    #[cfg(unix)]
    #[test]
    fn a_ninth_session_is_rejected_with_a_clear_error() {
        let scratch = Scratch::new("cap");
        let script = write_script(&scratch, "fake-shell.sh", FAKE_IDLE);
        let collected = Arc::new(Collected::default());
        let sessions = ShellSessions::new();

        let mut ids = Vec::new();
        for index in 0..MAX_SESSIONS {
            let session = sessions
                .open_with(binary(&script), &[], &scratch.dir, 80, 24, collected.sink())
                .unwrap_or_else(|err| panic!("session {index} must open: {err}"));
            ids.push(session.session_id);
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), MAX_SESSIONS, "every session gets its own id");

        let rejected =
            sessions.open_with(binary(&script), &[], &scratch.dir, 80, 24, collected.sink());
        match rejected {
            Err(ShellError::TooMany(cap)) => assert_eq!(cap, MAX_SESSIONS),
            other => panic!("the ninth session must be refused, got: {other:?}"),
        }
        let message = ShellError::TooMany(MAX_SESSIONS).to_string();
        assert!(
            message.contains("terminal") && message.contains(&MAX_SESSIONS.to_string()),
            "the error names the cap so the tab can say it: {message}"
        );

        // Closing one frees exactly one slot.
        assert!(sessions.close(&ids[0]));
        sessions
            .open_with(binary(&script), &[], &scratch.dir, 80, 24, collected.sink())
            .expect("a freed slot is reusable");
    }

    // ── the wire shape (S2) ─────────────────────────────────────────────

    // The event and the session are camelCase, so the TS contract mirrors them
    // field by field.
    #[test]
    fn serialized_payloads_are_camel_case() {
        let data = serde_json::to_value(ShellOutputEvent {
            session_id: "shell-1".to_string(),
            stream: ShellOutputStream::Data,
            data: Some("$ ".to_string()),
            exit_code: None,
        })
        .expect("serialize");
        assert_eq!(
            data,
            serde_json::json!({ "sessionId": "shell-1", "stream": "data", "data": "$ " })
        );

        let exit = serde_json::to_value(ShellOutputEvent {
            session_id: "shell-1".to_string(),
            stream: ShellOutputStream::Exit,
            data: None,
            exit_code: Some(3),
        })
        .expect("serialize");
        assert_eq!(
            exit,
            serde_json::json!({ "sessionId": "shell-1", "stream": "exit", "exitCode": 3 })
        );

        let session = serde_json::to_value(ShellSession {
            session_id: "shell-1".to_string(),
            pid: Some(42),
            shell: "/bin/zsh".to_string(),
        })
        .expect("serialize");
        assert_eq!(
            session,
            serde_json::json!({ "sessionId": "shell-1", "pid": 42, "shell": "/bin/zsh" })
        );

        // A platform that reports no pid keeps the key and sends `null`, so the
        // TS contract's `pid: number | null` is never `undefined`.
        let no_pid = serde_json::to_value(ShellSession {
            session_id: "shell-1".to_string(),
            pid: None,
            shell: "/bin/zsh".to_string(),
        })
        .expect("serialize");
        assert_eq!(
            no_pid,
            serde_json::json!({ "sessionId": "shell-1", "pid": null, "shell": "/bin/zsh" })
        );
    }

    // ── shell resolution (S2) ───────────────────────────────────────────

    // S2: the human's own shell wins when `$SHELL` names something real.
    #[test]
    fn resolve_shell_prefers_the_users_shell() {
        let shell = resolve_shell_with(Some("/opt/homebrew/bin/fish".to_string()), |_| true);
        assert_eq!(shell, "/opt/homebrew/bin/fish");
    }

    // S2: a stale `$SHELL` (a shell that was uninstalled) must not produce a
    // terminal that cannot start: the documented order keeps going.
    #[test]
    fn resolve_shell_falls_back_to_zsh_then_bash() {
        let zsh_only = resolve_shell_with(Some("/gone/old-shell".to_string()), |path| {
            path == Path::new("/bin/zsh")
        });
        assert_eq!(zsh_only, "/bin/zsh", "zsh is the first fallback");

        let bash_only = resolve_shell_with(None, |path| path == Path::new("/bin/bash"));
        assert_eq!(bash_only, "/bin/bash", "bash is the second fallback");
    }

    // S2: with no `$SHELL` and a system where neither fallback exists, the
    // resolution still names a program; the spawn is what reports the failure,
    // with its own error, instead of an empty command.
    #[test]
    fn resolve_shell_names_the_last_candidate_when_none_exists() {
        let shell = resolve_shell_with(None, |_| false);
        assert_eq!(shell, "/bin/bash");
    }

    // The resolution the app actually uses answers something usable here, so the
    // contract above is not only satisfied by injection.
    #[cfg(unix)]
    #[test]
    fn resolve_shell_on_this_machine_answers_an_absolute_shell() {
        let shell = resolve_shell();
        assert!(
            Path::new(&shell).is_absolute(),
            "a shell path is absolute: {shell:?}"
        );
    }
}
