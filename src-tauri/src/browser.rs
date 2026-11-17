// F3 of `odd/tasks/tool-drawer-and-mcp.md` (S4, S6): the browser the agent drives.
//
// Design notes:
// - **Launch, never attach** (decision 3 of the feature doc). Nabla starts its
//   own Chromium with a dedicated `--user-data-dir` under the app data
//   directory, so the human's cookies, sessions and extensions are never in
//   reach of the agent. A browser that is already running is never reused: it
//   was started with somebody else's profile and probably without a debugging
//   port.
// - **Chromium + CDP only** (S6). Safari and Firefox cannot be driven over CDP,
//   so they are not candidates at all: an empty detection result is what turns
//   the drawer tab into "disabled with a reason", with `chrome-devtools-mcp`
//   (F0) as the recorded alternative.
// - **HTTP only, no websockets.** `/json/version` proves readiness,
//   `/json/list` is what the browser has open, and `/json/new?<url>` navigates.
//   That is the whole surface this module needs, and it never speaks the
//   websocket protocol the agent's own harness uses.
// - **No HTTP client dependency.** The three endpoints are loopback requests
//   with `Connection: close`, so they are written over `TcpStream` here.
//   `reqwest` is async-only in this crate (the `blocking` feature would have to
//   be pulled in, and its blocking client panics inside a runtime), and the
//   readiness check is a synchronous poll loop. Keeping the request here keeps
//   the module testable against a fake server with no runtime at all.
// - **The registry, not the port, is the handle.** The webview addresses a
//   session by id; the port and the pid stay in Rust so a tab never has to know
//   how a browser is reached.
//
// Every payload serializes camelCase so the TS contracts mirror it field by
// field, like the rest of the seam (`commands.rs`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::process;

/// The only engine that exposes CDP to a process Nabla owns (S6). Safari and
/// Firefox are deliberately absent, so the field is a constant with one value
/// rather than an enum with an unsupported variant.
pub const CHROMIUM_FAMILY: &str = "chromium";

/// How many browsers may run at once. Two is enough for the intended use (the
/// agent's tab plus a second look at a different workspace); the cap exists so a
/// stuck UI cannot leak a process tree per click.
pub const MAX_SESSIONS: usize = 2;

/// How long a freshly spawned Chromium gets to answer `/json/version`. A cold
/// start with a brand new profile is a few seconds; fifteen is generous without
/// hanging a launch forever.
pub const READY_TIMEOUT: Duration = Duration::from_secs(15);

/// Interval between readiness probes.
pub const READY_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Dedicated profile, relative to the app data directory. Never the user's
/// default profile: that is the whole point of launching our own instance.
pub const PROFILE_DIR_NAME: &str = "browser-profile";

/// Deadline of one DevTools HTTP call. The endpoints are local and answer from
/// memory, so a slow answer means the browser is gone.
const HTTP_TIMEOUT: Duration = Duration::from_secs(2);

/// Grace given to a browser to leave after the polite terminate signal before
/// the force kill. Same bound as the other kill paths Nabla owns.
const KILL_GRACE: Duration = Duration::from_millis(500);

/// Cap on a DevTools response body. `/json/list` of a normal browser is a few
/// kilobytes; the cap keeps a hostile or broken endpoint from filling memory.
const MAX_BODY: usize = 1 << 20;

/// One Chromium-family executable found on this machine (S6).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCandidate {
    /// Human name of the product, as the UI shows it ("Google Chrome").
    pub name: String,
    /// Absolute path of the executable that is launched.
    pub path: String,
    /// Always `chromium` today; kept in the payload so a future engine never
    /// has to guess what it is looking at.
    pub family: String,
}

/// A browser Nabla started: what the webview learns when it launches one.
///
/// The id lives in [`BrowserStarted`] because a port and a pid do not identify a
/// session: the frontend addresses the registry, then the registry answers the
/// port (requirement 7).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSession {
    /// Loopback port the DevTools endpoints answer on.
    pub port: u16,
    /// Session leader of the browser process tree, so a kill can reach it.
    pub pid: u32,
    /// The product that was launched, so the tab can show it.
    pub browser_name: String,
    /// The dedicated profile directory, created before the spawn.
    pub profile_dir: String,
}

/// What `browser_launch` answers: the session id plus the session, flattened so
/// the wire shape is one flat camelCase object
/// (`{ sessionId, port, pid, browserName, profileDir }`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserStarted {
    pub session_id: String,
    #[serde(flatten)]
    pub session: BrowserSession,
}

/// One thing the browser has open, from `GET /json/list`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTarget {
    /// DevTools target id. It is the only field required to keep an entry: a row
    /// the UI cannot address is noise, so it is dropped.
    pub id: String,
    /// The DevTools `type` (`page`, `iframe`, `service_worker`, …), lowercased,
    /// or `other` when the endpoint did not say. Sentinel value, so the UI can
    /// paint pages first and fold the rest.
    pub kind: String,
    pub url: String,
    pub title: String,
}

/// `GET /json/version`. Only `Browser` is read: it proves the endpoint is a
/// DevTools server and names the instance ("Chrome/126.0.6478.127"). It never
/// crosses the seam, so it is not part of the TS contract.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct DevToolsVersion {
    #[serde(rename = "Browser")]
    pub browser: String,
}

/// Failures of the browser, typed inside Rust and rendered as a readable string
/// when they cross the Tauri bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserError {
    /// No Chromium-family executable was found (S6: Safari/Firefox only).
    NotDetected,
    /// `MAX_SESSIONS` browsers are already running.
    TooMany(usize),
    /// The dedicated profile directory could not be created.
    Profile(String),
    /// No loopback port could be reserved.
    Port(String),
    /// The executable could not be started.
    Spawn(String),
    /// Nothing answered `/json/version` in time; the child was killed.
    NotReady { port: u16, waited: Duration },
    /// The session id is not in the registry (already closed, or never here).
    UnknownSession(String),
    /// Only http and https urls are opened, and only printable ASCII ones.
    InvalidUrl(String),
}

impl std::fmt::Display for BrowserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrowserError::NotDetected => write!(
                f,
                "browser: no Chromium-based browser (Chrome, Edge, Brave, Chromium, BrowserOS) was found; Safari and Firefox cannot be driven over CDP"
            ),
            BrowserError::TooMany(cap) => write!(
                f,
                "browser: {cap} browsers are already running; close one before launching another"
            ),
            BrowserError::Profile(message) => {
                write!(f, "browser: could not prepare the profile directory: {message}")
            }
            BrowserError::Port(message) => {
                write!(f, "browser: could not reserve a loopback port: {message}")
            }
            BrowserError::Spawn(message) => write!(f, "browser: could not start the browser: {message}"),
            BrowserError::NotReady { port, waited } => write!(
                f,
                "browser: nothing answered http://127.0.0.1:{port}/json/version within {}s; the browser was stopped",
                waited.as_secs()
            ),
            BrowserError::UnknownSession(id) => write!(f, "browser: no session {id} is running"),
            BrowserError::InvalidUrl(url) => {
                write!(f, "browser: only printable http and https urls can be opened: {url}")
            }
        }
    }
}

impl std::error::Error for BrowserError {}

// ── detection (S6) ─────────────────────────────────────────────────────────

/// One product to look for, with its candidate executables in the order they are
/// tried (a Linux distribution may install `chromium` or `chromium-browser`).
struct BrowserLocation {
    name: &'static str,
    /// Relative to the root `detect_browsers_in` probes. Relative on purpose:
    /// the platform table then stays testable against a scratch directory.
    relative: &'static [&'static str],
}

/// The order the requirement states: Chrome first, then Edge, Brave, Chromium
/// and BrowserOS. First match wins for the launch.
#[cfg(target_os = "macos")]
const BROWSER_LOCATIONS: &[BrowserLocation] = &[
    BrowserLocation {
        name: "Google Chrome",
        relative: &["Applications/Google Chrome.app/Contents/MacOS/Google Chrome"],
    },
    BrowserLocation {
        name: "Microsoft Edge",
        relative: &["Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"],
    },
    BrowserLocation {
        name: "Brave Browser",
        relative: &["Applications/Brave Browser.app/Contents/MacOS/Brave Browser"],
    },
    BrowserLocation {
        name: "Chromium",
        relative: &["Applications/Chromium.app/Contents/MacOS/Chromium"],
    },
    BrowserLocation {
        name: "BrowserOS",
        relative: &["Applications/BrowserOS.app/Contents/MacOS/BrowserOS"],
    },
];

#[cfg(target_os = "linux")]
const BROWSER_LOCATIONS: &[BrowserLocation] = &[
    BrowserLocation {
        name: "Google Chrome",
        relative: &["usr/bin/google-chrome", "usr/bin/google-chrome-stable"],
    },
    BrowserLocation {
        name: "Microsoft Edge",
        relative: &["usr/bin/microsoft-edge", "usr/bin/microsoft-edge-stable"],
    },
    BrowserLocation {
        name: "Brave Browser",
        relative: &["usr/bin/brave-browser"],
    },
    BrowserLocation {
        name: "Chromium",
        relative: &["usr/bin/chromium", "usr/bin/chromium-browser"],
    },
    BrowserLocation {
        name: "BrowserOS",
        relative: &["usr/bin/browseros"],
    },
];

#[cfg(windows)]
const BROWSER_LOCATIONS: &[BrowserLocation] = &[
    BrowserLocation {
        name: "Google Chrome",
        relative: &[
            "Program Files/Google/Chrome/Application/chrome.exe",
            "Program Files (x86)/Google/Chrome/Application/chrome.exe",
        ],
    },
    BrowserLocation {
        name: "Microsoft Edge",
        relative: &[
            "Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
            "Program Files/Microsoft/Edge/Application/msedge.exe",
        ],
    },
    BrowserLocation {
        name: "Brave Browser",
        relative: &["Program Files/BraveSoftware/Brave-Browser/Application/brave.exe"],
    },
    BrowserLocation {
        name: "Chromium",
        relative: &["Program Files/Chromium/Application/chrome.exe"],
    },
    BrowserLocation {
        name: "BrowserOS",
        relative: &["Program Files/BrowserOS/Application/BrowserOS.exe"],
    },
];

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
const BROWSER_LOCATIONS: &[BrowserLocation] = &[];

/// Every Chromium-family browser installed on this machine, in the documented
/// order, Chrome first. Empty means "the tab stays disabled with a reason" (S6),
/// which is not an error.
pub fn detect_browsers() -> Vec<BrowserCandidate> {
    detect_browsers_in(&filesystem_root())
}

/// Same probe against an injected root. Production passes `/` (or the system
/// drive on Windows); tests pass a scratch directory, so detection is provable
/// on an empty filesystem without touching the real one.
pub fn detect_browsers_in(root: &Path) -> Vec<BrowserCandidate> {
    detect_in(root, BROWSER_LOCATIONS)
}

fn detect_in(root: &Path, locations: &[BrowserLocation]) -> Vec<BrowserCandidate> {
    let mut found = Vec::new();
    for location in locations {
        let executable = location
            .relative
            .iter()
            .map(|relative| root.join(relative))
            .find(|candidate| is_executable(candidate));
        if let Some(path) = executable {
            found.push(BrowserCandidate {
                name: location.name.to_string(),
                path: path.to_string_lossy().into_owned(),
                family: CHROMIUM_FAMILY.to_string(),
            });
        }
    }
    found
}

/// The root the real probe starts from: the filesystem root on the Unixes and
/// the system drive on Windows.
fn filesystem_root() -> PathBuf {
    #[cfg(windows)]
    {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
        PathBuf::from(format!("{drive}/"))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/")
    }
}

/// An executable file. A directory whose name matches a browser is not one, and
/// on Unix a file without any execute bit is not launchable either.
fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}

// ── launch ─────────────────────────────────────────────────────────────────

/// The argv of a dedicated instance: the documented flags, in the documented
/// order, and a blank page so the browser opens ready to navigate.
pub fn chromium_argv(port: u16, profile_dir: &Path) -> Vec<String> {
    vec![
        format!("--remote-debugging-port={port}"),
        format!("--user-data-dir={}", profile_dir.display()),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-background-networking".to_string(),
        // A blank tab, so the browser opens ready to navigate instead of
        // restoring whatever the profile last had.
        "about:blank".to_string(),
    ]
}

/// A free loopback port: bind `127.0.0.1:0`, read the port the kernel chose, drop
/// the listener. There is a small race between the drop and Chromium's bind; the
/// readiness poll is what turns that race into a readable error instead of a
/// silent failure.
fn pick_free_port() -> Result<u16, BrowserError> {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .map_err(|error| BrowserError::Port(error.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|error| BrowserError::Port(error.to_string()))?
        .port();
    drop(listener);
    Ok(port)
}

/// Spawns the browser and waits for its DevTools endpoint. The child handle is
/// returned so the registry can close and reap it.
fn start(
    workspace: &Path,
    app_data_dir: &Path,
    timeout: Duration,
) -> Result<(BrowserSession, Child), BrowserError> {
    // The first candidate is Chrome when it is installed, then Edge, Brave,
    // Chromium and BrowserOS: the order the requirement states.
    let candidates = detect_browsers();
    let browser = candidates.first().ok_or(BrowserError::NotDetected)?;
    start_binary(
        Path::new(&browser.path),
        &browser.name,
        workspace,
        app_data_dir,
        timeout,
    )
}

/// Spawns one known executable. Split from [`start`] so a test can point it at a
/// fake browser without inventing a detection table.
fn start_binary(
    binary: &Path,
    name: &str,
    workspace: &Path,
    app_data_dir: &Path,
    timeout: Duration,
) -> Result<(BrowserSession, Child), BrowserError> {
    let port = pick_free_port()?;
    let profile_dir = app_data_dir.join(PROFILE_DIR_NAME);
    std::fs::create_dir_all(&profile_dir)
        .map_err(|error| BrowserError::Profile(format!("{}: {error}", profile_dir.display())))?;

    let mut command = Command::new(binary);
    command.args(chromium_argv(port, &profile_dir));
    // The workspace is the browser's working directory: anything it resolves
    // relative to the process lands in the project, never in the app data dir.
    command.current_dir(workspace);
    // No inherited pipes. A GUI process that kept the host's stdout open would
    // hold the handle for as long as it runs.
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group (pgid == pid) so one signal reaches the
        // renderers and helpers the browser spawns, exactly like pi's child.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // S4: no console window flashes when a browser starts.
        // SAFETY: a documented creation flag, no memory or pointer effects.
        unsafe {
            command.creation_flags(process::CREATE_NO_WINDOW);
        }
    }

    let mut child = command
        .spawn()
        .map_err(|error| BrowserError::Spawn(format!("{}: {error}", binary.display())))?;
    let session = BrowserSession {
        port,
        pid: child.id(),
        browser_name: name.to_string(),
        profile_dir: profile_dir.to_string_lossy().into_owned(),
    };
    match wait_until_ready(port, timeout, READY_POLL_INTERVAL) {
        Ok(_version) => Ok((session, child)),
        Err(error) => {
            // A browser that never answered is not usable, and must not be left
            // as an orphan window with a debugging port open.
            terminate(&mut child);
            Err(error)
        }
    }
}

/// Polls `GET /json/version` until the endpoint answers or the deadline passes.
fn wait_until_ready(
    port: u16,
    timeout: Duration,
    poll: Duration,
) -> Result<DevToolsVersion, BrowserError> {
    let deadline = Instant::now() + timeout;
    loop {
        let replied = request(port, "GET", "/json/version", HTTP_TIMEOUT.min(timeout))
            .ok()
            .filter(|reply| reply.status == 200)
            .and_then(|reply| parse_version(&reply.body));
        if let Some(version) = replied {
            return Ok(version);
        }
        if Instant::now() >= deadline {
            return Err(BrowserError::NotReady { port, waited: timeout });
        }
        std::thread::sleep(poll);
    }
}

/// Escalating kill, shared by every close path: polite terminate, then force.
///
/// The browser was spawned in its own process group (see `start_binary`), so the
/// shared process helpers reach the renderer and helper children too — killing
/// only the launcher would leave a window nobody can address.
fn terminate(child: &mut Child) {
    if exited(child) {
        return;
    }
    process::signal_terminate(child.id());
    if wait_for_exit(child, KILL_GRACE) {
        return;
    }
    process::signal_kill(child.id());
    let _ = wait_for_exit(child, KILL_GRACE);
}

/// Whether the child has already left. `try_wait` reaps it, so a `true` here
/// means no zombie is left behind.
fn exited(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(Some(_)))
}

/// Waits up to `grace` for the child to be reaped.
fn wait_for_exit(child: &mut Child, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        if exited(child) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

// ── DevTools HTTP endpoints ────────────────────────────────────────────────

/// What the browser has open (`GET /json/list`).
///
/// An unreachable endpoint is not an error: the browser may have just exited
/// because the human closed its window, and an empty list is what the tab paints.
pub fn targets(port: u16) -> Vec<BrowserTarget> {
    match request(port, "GET", "/json/list", HTTP_TIMEOUT) {
        Ok(reply) if reply.status == 200 => parse_targets(&reply.body),
        _ => Vec::new(),
    }
}

/// Opens `url` in a new tab (`PUT /json/new?<url>`). Chrome requires PUT and
/// answers 405 to anything else, so a 405 is retried once with GET.
///
/// `Ok(None)` means the endpoint refused or could not be reached; the caller
/// shows that as "could not open", not as a crash. A url that is not printable
/// http(s) is rejected before any request.
pub fn open_url(port: u16, url: &str) -> Result<Option<BrowserTarget>, BrowserError> {
    let url = validated_url(url)?;
    let path = format!("/json/new?{url}");
    let Ok(reply) = request(port, "PUT", &path, HTTP_TIMEOUT) else {
        return Ok(None);
    };
    // Current Chrome only accepts PUT and answers 405 to anything else, while an
    // older build only took GET. One retry covers both, so the caller never has
    // to know which one it is talking to.
    let reply = if reply.status == 405 {
        request(port, "GET", &path, HTTP_TIMEOUT).unwrap_or(reply)
    } else {
        reply
    };
    if !(200..300).contains(&reply.status) {
        return Ok(None);
    }
    Ok(parse_target_body(&reply.body))
}

/// Accepts only http(s) with a host and only printable ASCII, because the url is
/// appended verbatim to the request target. A url with a space, a control
/// character or non-ASCII bytes must be percent-encoded by the caller.
fn validated_url(raw: &str) -> Result<&str, BrowserError> {
    let url = raw.trim();
    if url.is_empty() || !url.chars().all(|character| character.is_ascii_graphic()) {
        return Err(BrowserError::InvalidUrl(raw.to_string()));
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(BrowserError::InvalidUrl(raw.to_string()));
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(BrowserError::InvalidUrl(raw.to_string()));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err(BrowserError::InvalidUrl(raw.to_string()));
    }
    Ok(url)
}

fn parse_version(body: &str) -> Option<DevToolsVersion> {
    let version: DevToolsVersion = serde_json::from_str(body).ok()?;
    if version.browser.trim().is_empty() {
        return None;
    }
    Some(version)
}

fn parse_targets(body: &str) -> Vec<BrowserTarget> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    items.iter().filter_map(parse_target).collect()
}

fn parse_target(value: &serde_json::Value) -> Option<BrowserTarget> {
    // The id is the only required field: a row the UI cannot address is noise.
    let id = value.get("id").and_then(serde_json::Value::as_str)?.trim();
    if id.is_empty() {
        return None;
    }
    Some(BrowserTarget {
        id: id.to_string(),
        kind: kind_of(value.get("type").and_then(serde_json::Value::as_str)),
        url: field_text(value.get("url")),
        title: field_text(value.get("title")),
    })
}

/// The single target `PUT /json/new` answers.
fn parse_target_body(body: &str) -> Option<BrowserTarget> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| parse_target(&value))
}

fn kind_of(raw: Option<&str>) -> String {
    match raw.map(str::trim).filter(|kind| !kind.is_empty()) {
        Some(kind) => kind.to_ascii_lowercase(),
        None => "other".to_string(),
    }
}

/// A string field, or an empty string when it is missing or not a string. The
/// target still exists, so dropping it over a missing title would hide a page the
/// human can see.
fn field_text(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One reply of a DevTools endpoint.
struct HttpReply {
    status: u16,
    body: String,
}

/// One loopback request with `Connection: close`, answered in full.
fn request(port: u16, method: &str, path: &str, timeout: Duration) -> Result<HttpReply, String> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream =
        TcpStream::connect_timeout(&address, timeout).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    // `Content-Length: 0` keeps a PUT well formed; `Connection: close` lets the
    // read below run to EOF instead of parsing a keep-alive framing.
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(head.as_bytes())
        .map_err(|error| error.to_string())?;

    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                raw.extend_from_slice(&chunk[..read]);
                if raw.len() >= MAX_BODY {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    parse_http(&raw)
}

fn parse_http(raw: &[u8]) -> Result<HttpReply, String> {
    let Some(split) = raw.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Err("malformed response: no header terminator".to_string());
    };
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut body = raw[split + 4..].to_vec();

    let mut lines = head.lines();
    let Some(status_line) = lines.next() else {
        return Err("malformed response: no status line".to_string());
    };
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("malformed response status line: {status_line}"))?;

    let mut chunked = false;
    let mut length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "content-length" => length = value.parse::<usize>().ok(),
            "transfer-encoding" => chunked = value.to_ascii_lowercase().contains("chunked"),
            _ => {}
        }
    }

    if chunked {
        body = dechunk(&body);
    } else if let Some(length) = length {
        body.truncate(length.min(body.len()));
    }
    Ok(HttpReply {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// Decodes a chunked body. The DevTools endpoints normally answer with
/// `Content-Length`, but a tolerant reader costs a few lines and never chokes on
/// a proxy in between.
fn dechunk(body: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    let mut rest = body;
    while let Some(end) = rest.windows(2).position(|window| window == b"\r\n") {
        let size_line = String::from_utf8_lossy(&rest[..end]).into_owned();
        // A chunk size may carry extensions after a `;`.
        let size_text = size_line.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_text, 16) else {
            break;
        };
        rest = &rest[end + 2..];
        if size == 0 {
            break;
        }
        if rest.len() < size {
            decoded.extend_from_slice(rest);
            break;
        }
        decoded.extend_from_slice(&rest[..size]);
        rest = &rest[size..];
        if rest.starts_with(b"\r\n") {
            rest = &rest[2..];
        }
    }
    decoded
}

// ── the registry ───────────────────────────────────────────────────────────

/// One launched browser, with the handle that can reap it.
struct Entry {
    session: BrowserSession,
    child: Child,
}

struct Inner {
    entries: Mutex<HashMap<String, Entry>>,
    /// Launches in flight. Counted so two concurrent launches cannot both pass
    /// the cap check and then both insert.
    in_flight: Mutex<usize>,
    next_id: AtomicUsize,
}

/// Every browser Nabla started, addressed by id (requirement 7). Cloning the
/// handle shares the registry; the last clone out kills every child it still
/// owns, so a browser never outlives the app.
#[derive(Clone)]
pub struct BrowserSessions {
    inner: Arc<Inner>,
}

impl BrowserSessions {
    pub fn new() -> BrowserSessions {
        BrowserSessions {
            inner: Arc::new(Inner {
                entries: Mutex::new(HashMap::new()),
                in_flight: Mutex::new(0),
                next_id: AtomicUsize::new(1),
            }),
        }
    }

    /// Launches a browser and registers it. The cap is checked before anything
    /// is spawned, so a refused launch leaves no process behind.
    pub fn launch(
        &self,
        workspace: &Path,
        app_data_dir: &Path,
    ) -> Result<BrowserStarted, BrowserError> {
        let _slot = self.reserve()?;
        let (session, child) = start(workspace, app_data_dir, READY_TIMEOUT)?;
        let session_id = format!(
            "browser-{}",
            self.inner.next_id.fetch_add(1, Ordering::Relaxed)
        );
        lock(&self.inner.entries).insert(
            session_id.clone(),
            Entry {
                session: session.clone(),
                child,
            },
        );
        Ok(BrowserStarted {
            session_id,
            session,
        })
    }

    /// Takes one of the `MAX_SESSIONS` slots or refuses. The slot is held for the
    /// whole launch and released when it goes out of scope, so two concurrent
    /// launches can never both pass the check and then both insert.
    fn reserve(&self) -> Result<Reservation<'_>, BrowserError> {
        let mut in_flight = lock(&self.inner.in_flight);
        let live = lock(&self.inner.entries).len();
        if live + *in_flight >= MAX_SESSIONS {
            return Err(BrowserError::TooMany(MAX_SESSIONS));
        }
        *in_flight += 1;
        drop(in_flight);
        Ok(Reservation { inner: &self.inner })
    }

    /// One session, by id.
    pub fn session(&self, session_id: &str) -> Option<BrowserSession> {
        lock(&self.inner.entries)
            .get(session_id)
            .map(|entry| entry.session.clone())
    }

    pub fn targets(&self, session_id: &str) -> Result<Vec<BrowserTarget>, BrowserError> {
        let session = self
            .session(session_id)
            .ok_or_else(|| BrowserError::UnknownSession(session_id.to_string()))?;
        Ok(targets(session.port))
    }

    pub fn open_url(
        &self,
        session_id: &str,
        url: &str,
    ) -> Result<Option<BrowserTarget>, BrowserError> {
        let session = self
            .session(session_id)
            .ok_or_else(|| BrowserError::UnknownSession(session_id.to_string()))?;
        open_url(session.port, url)
    }

    /// Closes one session by pid. `false` means the id is unknown, which is not
    /// an error: a tab that closes twice is normal.
    pub fn close(&self, session_id: &str) -> bool {
        let Some(mut entry) = lock(&self.inner.entries).remove(session_id) else {
            return false;
        };
        // The entry is already out of the map, so no other call can reach this
        // child while it is being stopped.
        terminate(&mut entry.child);
        true
    }

}

impl Default for BrowserSessions {
    fn default() -> Self {
        Self::new()
    }
}

/// One reserved launch slot, released when it goes out of scope.
struct Reservation<'a> {
    inner: &'a Inner,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let mut in_flight = lock(&self.inner.in_flight);
        *in_flight = in_flight.saturating_sub(1);
    }
}

impl Inner {
    /// Kills every child still registered. This is the drop path, so the app
    /// going away never leaves a browser — or its debugging port — behind.
    fn close_all(&self) {
        let drained = std::mem::take(&mut *lock(&self.entries));
        for (_, mut entry) in drained {
            terminate(&mut entry.child);
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.close_all();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Scratch directory in the system temp dir (never the worktree), removed on
    /// drop.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let mut dir = std::env::temp_dir();
            dir.push(format!(
                "nabla_browser_{}_{}_{tag}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
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

    // ── detection ──────────────────────────────────────────────────────────

    /// A fixture table, so detection is provable without depending on which
    /// browsers the machine running the tests happens to have installed.
    const FIXTURE_LOCATIONS: &[BrowserLocation] = &[
        BrowserLocation {
            name: "Google Chrome",
            relative: &["apps/chrome"],
        },
        BrowserLocation {
            name: "Microsoft Edge",
            relative: &["apps/edge"],
        },
        BrowserLocation {
            name: "Brave Browser",
            relative: &["apps/brave"],
        },
        BrowserLocation {
            name: "Chromium",
            relative: &["apps/chromium", "apps/chromium-beta"],
        },
        BrowserLocation {
            name: "BrowserOS",
            relative: &["apps/browseros"],
        },
    ];

    #[cfg(unix)]
    fn write_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(path, "#!/bin/sh\nexit 0\n").expect("write executable");
        let mut permissions = std::fs::metadata(path).expect("stat").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("chmod");
    }

    #[cfg(windows)]
    fn write_executable(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(path, "").expect("write executable");
    }

    /// S6: no browser installed is an answer, not a panic. The probe walks a root
    /// that has nothing in it.
    #[test]
    fn detection_returns_nothing_on_an_empty_filesystem() {
        let scratch = Scratch::new("detect_empty");

        let found = detect_browsers_in(&scratch.dir);

        assert!(found.is_empty(), "an empty root must detect nothing");
    }

    /// S6: only real executables, in the documented order (Chrome first), with
    /// the first working candidate path of each product.
    #[test]
    fn detection_lists_only_existing_executables_in_order() {
        let scratch = Scratch::new("detect_order");
        let root = &scratch.dir;
        write_executable(&root.join("apps/chrome"));
        // A second candidate of the same product is only used when the first is
        // missing, so Chromium must resolve to `chromium-beta` here.
        write_executable(&root.join("apps/chromium-beta"));
        write_executable(&root.join("apps/browseros"));
        // A directory is not an executable, and neither is a plain file.
        std::fs::create_dir_all(root.join("apps/edge")).expect("mkdir edge");
        if let Some(parent) = root.join("apps/brave").parent() {
            std::fs::create_dir_all(parent).expect("mkdir apps");
        }
        std::fs::write(root.join("apps/brave"), "not a program").expect("write brave");

        let found = detect_in(root, FIXTURE_LOCATIONS);

        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        let expected: &[&str] = if cfg!(unix) {
            // Without an execute bit the Brave file is not launchable either.
            &["Google Chrome", "Chromium", "BrowserOS"]
        } else {
            &["Google Chrome", "Brave Browser", "Chromium", "BrowserOS"]
        };
        assert_eq!(names, expected, "detection order and filtering");
        assert_eq!(
            found[0].path,
            root.join("apps/chrome").to_string_lossy(),
            "the reported path is the executable that would be launched"
        );
        assert_eq!(
            found[1].path,
            root.join("apps/chromium-beta").to_string_lossy(),
            "the first existing candidate of a product wins"
        );
        for candidate in &found {
            assert_eq!(candidate.family, "chromium");
        }
    }

    /// S6: the real table keeps the documented order and only names Chromium
    /// engines.
    #[test]
    fn the_platform_table_is_chrome_first_and_all_chromium() {
        let names: Vec<&str> = BROWSER_LOCATIONS.iter().map(|entry| entry.name).collect();
        if !BROWSER_LOCATIONS.is_empty() {
            assert_eq!(names[0], "Google Chrome", "Chrome is tried first");
        }
        let expected = [
            "Google Chrome",
            "Microsoft Edge",
            "Brave Browser",
            "Chromium",
            "BrowserOS",
        ];
        assert_eq!(names, expected, "the documented detection order");

        let scratch = Scratch::new("detect_real_table");
        for entry in BROWSER_LOCATIONS {
            write_executable(&scratch.dir.join(entry.relative[0]));
        }
        let found = detect_browsers_in(&scratch.dir);
        assert_eq!(found.len(), BROWSER_LOCATIONS.len());
        assert!(found.iter().all(|c| c.family == "chromium"));
    }

    // ── argv ───────────────────────────────────────────────────────────────

    #[test]
    fn chromium_argv_carries_exactly_the_documented_flags() {
        let profile = Path::new("/tmp/nabla-test-profile");

        let argv = chromium_argv(9333, profile);

        assert_eq!(
            argv,
            vec![
                "--remote-debugging-port=9333".to_string(),
                format!("--user-data-dir={}", profile.display()),
                "--no-first-run".to_string(),
                "--no-default-browser-check".to_string(),
                "--disable-background-networking".to_string(),
                "about:blank".to_string(),
            ]
        );
    }

    // ── port ───────────────────────────────────────────────────────────────

    #[test]
    fn the_free_port_picker_returns_a_bindable_port() {
        let port = pick_free_port().expect("a free port");

        assert_ne!(port, 0);
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
        assert!(
            listener.is_ok(),
            "the picked port must still be bindable, got {port}"
        );
    }

    // ── /json/version ──────────────────────────────────────────────────────

    const VERSION_FIXTURE: &str = r#"{
        "Browser": "Chrome/126.0.6478.127",
        "Protocol-Version": "1.3",
        "User-Agent": "Mozilla/5.0",
        "V8-Version": "12.6.228.17",
        "WebKit-Version": "537.36",
        "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/browser/abc"
    }"#;

    #[test]
    fn the_version_fixture_is_parsed_and_malformed_ones_are_not() {
        assert_eq!(
            parse_version(VERSION_FIXTURE),
            Some(DevToolsVersion {
                browser: "Chrome/126.0.6478.127".to_string()
            })
        );
        for malformed in ["", "not json", "[]", "{}", r#"{"Browser": 5}"#, r#"{"Browser": "  "}"#] {
            assert_eq!(parse_version(malformed), None, "rejected: {malformed:?}");
        }
    }

    // ── /json/list ─────────────────────────────────────────────────────────

    const TARGETS_FIXTURE: &str = r#"[
        {
            "description": "",
            "devtoolsFrontendUrl": "/devtools/inspector.html?ws=127.0.0.1:9222/devtools/page/AAA",
            "id": "AAA",
            "title": "Example Domain",
            "type": "page",
            "url": "https://example.com/",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/AAA"
        },
        {
            "id": "BBB",
            "title": "Service Worker",
            "type": "service_worker",
            "url": "https://example.com/sw.js"
        },
        {
            "id": "CCC",
            "title": "",
            "type": "iframe",
            "url": "https://example.com/frame"
        },
        {
            "title": "no id",
            "type": "page",
            "url": "https://no-id.example/"
        },
        {
            "id": "   ",
            "type": "page",
            "url": "https://blank-id.example/"
        },
        "not an object",
        {
            "id": "DDD",
            "title": "no type"
        },
        {
            "id": "EEE",
            "title": "background",
            "type": "BACKGROUND_PAGE",
            "url": "chrome-extension://abc/bg.html"
        }
    ]"#;

    /// `type` becomes `kind`, everything stringy survives, and an entry the UI
    /// could not address (no id) or could not read (not an object) is dropped
    /// instead of failing the whole list.
    #[test]
    fn the_targets_fixture_maps_type_to_kind_and_skips_malformed_entries() {
        let targets = parse_targets(TARGETS_FIXTURE);

        let expected = vec![
            BrowserTarget {
                id: "AAA".to_string(),
                kind: "page".to_string(),
                url: "https://example.com/".to_string(),
                title: "Example Domain".to_string(),
            },
            BrowserTarget {
                id: "BBB".to_string(),
                kind: "service_worker".to_string(),
                url: "https://example.com/sw.js".to_string(),
                title: "Service Worker".to_string(),
            },
            BrowserTarget {
                id: "CCC".to_string(),
                kind: "iframe".to_string(),
                url: "https://example.com/frame".to_string(),
                title: String::new(),
            },
            BrowserTarget {
                id: "DDD".to_string(),
                kind: "other".to_string(),
                url: String::new(),
                title: "no type".to_string(),
            },
            BrowserTarget {
                id: "EEE".to_string(),
                kind: "background_page".to_string(),
                url: "chrome-extension://abc/bg.html".to_string(),
                title: "background".to_string(),
            },
        ];
        assert_eq!(targets, expected);
    }

    #[test]
    fn a_payload_that_is_not_a_target_list_parses_as_empty() {
        for malformed in ["", "{}", "not json", "[1, 2, 3]", r#"[{"id": "A"}]"#] {
            let targets = parse_targets(malformed);
            if malformed == r#"[{"id": "A"}]"# {
                assert_eq!(targets.len(), 1, "a minimal entry still has an id");
                assert_eq!(targets[0].kind, "other");
            } else {
                assert!(targets.is_empty(), "rejected: {malformed:?}");
            }
        }
    }

    /// The browser may be gone (the human closed the window); the tab paints an
    /// empty list instead of an error.
    #[test]
    fn targets_answers_empty_when_the_endpoint_is_unreachable() {
        let port = pick_free_port().expect("a free port");
        assert!(targets(port).is_empty());
    }

    // ── url validation ─────────────────────────────────────────────────────

    #[test]
    fn only_printable_http_urls_are_accepted() {
        for accepted in [
            "http://example.com/",
            "https://example.com/path?q=1#frag",
            "HTTPS://EXAMPLE.COM/",
            "http://127.0.0.1:8080/x",
        ] {
            assert_eq!(validated_url(accepted), Ok(accepted), "accepted: {accepted}");
        }
        for rejected in [
            "",
            "example.com",
            "http://",
            "https:///path",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "chrome://settings",
            "about:blank",
            "http://example.com/a b",
            "http://example.com/ñ",
        ] {
            assert!(
                matches!(validated_url(rejected), Err(BrowserError::InvalidUrl(_))),
                "rejected: {rejected:?}"
            );
        }
    }

    #[test]
    fn open_url_rejects_a_non_http_url_before_any_request() {
        let port = pick_free_port().expect("a free port");
        let error = open_url(port, "file:///etc/passwd").expect_err("must be rejected");
        assert!(matches!(error, BrowserError::InvalidUrl(_)));
    }

    // ── a fake DevTools endpoint: no real browser is ever spawned ──────────

    fn http_response(status: u16, body: &str) -> String {
        format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// Serves the queued replies, one connection each, and records the request
    /// line of every connection it answered.
    fn fake_endpoint(replies: Vec<String>) -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("bind fake endpoint");
        let port = listener.local_addr().expect("fake addr").port();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        std::thread::spawn(move || {
            for reply in replies {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut raw = Vec::new();
                let mut chunk = [0u8; 1024];
                while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(read) => raw.extend_from_slice(&chunk[..read]),
                        Err(_) => break,
                    }
                }
                if let Some(line) = String::from_utf8_lossy(&raw).lines().next() {
                    lock(&recorder).push(line.to_string());
                }
                let _ = stream.write_all(reply.as_bytes());
                let _ = stream.flush();
            }
        });
        (port, seen)
    }

    #[test]
    fn readiness_returns_the_version_when_the_endpoint_answers() {
        let (port, _seen) = fake_endpoint(vec![http_response(200, VERSION_FIXTURE)]);

        let version = wait_until_ready(port, Duration::from_secs(2), Duration::from_millis(20))
            .expect("a ready browser");

        assert_eq!(version.browser, "Chrome/126.0.6478.127");
    }

    #[test]
    fn readiness_times_out_when_nothing_answers() {
        let port = pick_free_port().expect("a free port");

        let started = Instant::now();
        let error = wait_until_ready(port, Duration::from_millis(300), Duration::from_millis(50))
            .expect_err("nothing is listening");
        let waited = started.elapsed();

        assert_eq!(
            error,
            BrowserError::NotReady {
                port,
                waited: Duration::from_millis(300)
            }
        );
        assert!(
            waited >= Duration::from_millis(300),
            "the poll loop must spend its whole budget, waited {waited:?}"
        );
        assert!(
            waited < Duration::from_secs(5),
            "the poll loop must not overshoot, waited {waited:?}"
        );
    }

    /// Chrome requires PUT on `/json/new`. A GET-only endpoint answers 405, and
    /// the retry has to keep the same target.
    #[test]
    fn open_url_puts_and_retries_once_with_get_on_405() {
        let target = r#"{"id": "NEW", "type": "page", "title": "Example Domain", "url": "https://example.com/"}"#;
        let (port, seen) = fake_endpoint(vec![
            http_response(405, ""),
            http_response(200, target),
        ]);

        let opened = open_url(port, "https://example.com/")
            .expect("a refused PUT falls back to GET")
            .expect("the opened target");

        assert_eq!(opened.id, "NEW");
        assert_eq!(opened.kind, "page");
        assert_eq!(opened.url, "https://example.com/");
        let requests = lock(&seen).clone();
        assert_eq!(
            requests,
            vec![
                "PUT /json/new?https://example.com/ HTTP/1.1".to_string(),
                "GET /json/new?https://example.com/ HTTP/1.1".to_string(),
            ]
        );
    }

    #[test]
    fn open_url_answers_none_when_the_endpoint_refuses() {
        let (port, _seen) = fake_endpoint(vec![http_response(500, "")]);
        assert_eq!(open_url(port, "https://example.com/"), Ok(None));

        let closed = pick_free_port().expect("a free port");
        assert_eq!(open_url(closed, "https://example.com/"), Ok(None));
    }

    #[test]
    fn a_chunked_reply_is_decoded() {
        let body = r#"{"Browser": "Chrome/1"}"#;
        let raw = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            body.len(),
            body
        );

        let reply = parse_http(raw.as_bytes()).expect("a well formed reply");

        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, body);
    }

    #[test]
    fn a_reply_without_a_header_terminator_is_an_error() {
        assert!(parse_http(b"HTTP/1.1 200 OK").is_err());
        assert!(parse_http(b"garbage\r\n\r\n").is_err());
    }

    // ── registry ───────────────────────────────────────────────────────────

    #[cfg(unix)]
    fn spawn_sleeper() -> Child {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg("while :; do sleep 1; done");
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        // Its own process group, exactly like a launched browser, so the
        // recorded kill path is the one production uses.
        command.process_group(0);
        command.spawn().expect("spawn sleeper")
    }

    #[cfg(unix)]
    fn process_alive(pid: u32) -> bool {
        // SAFETY: kill() only reads its two integer arguments.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    impl BrowserSessions {
        /// Registers a child the test already spawned, so the registry's cap,
        /// lookup and close paths are provable without launching a browser.
        fn register_for_test(&self, session: BrowserSession, child: Child) -> String {
            let id = format!(
                "browser-test-{}",
                self.inner.next_id.fetch_add(1, Ordering::Relaxed)
            );
            lock(&self.inner.entries).insert(
                id.clone(),
                Entry {
                    session,
                    child,
                },
            );
            id
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_registry_refuses_a_third_session_before_spawning_anything() {
        let sessions = BrowserSessions::new();
        let mut children = Vec::new();
        for index in 0..MAX_SESSIONS {
            let child = spawn_sleeper();
            children.push(child.id());
            sessions.register_for_test(
                BrowserSession {
                    port: 9000 + index as u16,
                    pid: child.id(),
                    browser_name: "Fake".to_string(),
                    profile_dir: String::new(),
                },
                child,
            );
        }

        let error = sessions
            .launch(Path::new("/"), Path::new("/tmp"))
            .expect_err("the cap is reached");

        assert_eq!(error, BrowserError::TooMany(MAX_SESSIONS));
        for pid in children {
            assert!(process_alive(pid), "a refused launch kills nothing");
        }
    }

    #[cfg(unix)]
    #[test]
    fn closing_by_id_terminates_the_child_and_forgets_the_session() {
        let sessions = BrowserSessions::new();
        let child = spawn_sleeper();
        let pid = child.id();
        let id = sessions.register_for_test(
            BrowserSession {
                port: 9001,
                pid,
                browser_name: "Fake".to_string(),
                profile_dir: String::new(),
            },
            child,
        );
        assert_eq!(sessions.session(&id).map(|s| s.pid), Some(pid));

        assert!(sessions.close(&id));

        assert!(!process_alive(pid), "the browser must be gone");
        assert!(sessions.session(&id).is_none(), "the id must be forgotten");
        assert!(!sessions.close(&id), "closing twice is not an error");
    }

    #[cfg(unix)]
    #[test]
    fn dropping_the_registry_kills_every_child_it_still_owns() {
        let sessions = BrowserSessions::new();
        let mut pids = Vec::new();
        for index in 0..2u16 {
            let child = spawn_sleeper();
            pids.push(child.id());
            sessions.register_for_test(
                BrowserSession {
                    port: 9100 + index,
                    pid: child.id(),
                    browser_name: "Fake".to_string(),
                    profile_dir: String::new(),
                },
                child,
            );
        }

        drop(sessions);

        for pid in pids {
            assert!(!process_alive(pid), "no browser may outlive the app");
        }
    }

    #[test]
    fn an_unknown_session_is_reported_not_guessed() {
        let sessions = BrowserSessions::new();

        assert!(sessions.session("nope").is_none());
        assert_eq!(
            sessions.targets("nope"),
            Err(BrowserError::UnknownSession("nope".to_string()))
        );
        assert_eq!(
            sessions.open_url("nope", "https://example.com/"),
            Err(BrowserError::UnknownSession("nope".to_string()))
        );
        assert!(!sessions.close("nope"));
    }

    #[test]
    fn a_registered_session_answers_its_targets() {
        let target = r#"[{"id": "AAA", "type": "page", "title": "Example", "url": "https://example.com/"}]"#;
        let (port, _seen) = fake_endpoint(vec![http_response(200, target)]);
        let sessions = BrowserSessions::new();
        // A session with no process: only the port is read on this path.
        let id = sessions.register_for_test(
            BrowserSession {
                port,
                pid: 0,
                browser_name: "Fake".to_string(),
                profile_dir: String::new(),
            },
            spawn_sleeper_fixture(),
        );

        let targets = sessions.targets(&id).expect("the session is known");

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, "AAA");
    }

    /// The registry holds a `Child`, so even a read-only test needs one. This is
    /// a child that has already exited, which is exactly the state a reaped
    /// browser is in and keeps the test platform-neutral.
    fn spawn_sleeper_fixture() -> Child {
        #[cfg(unix)]
        {
            let child = spawn_sleeper();
            let mut child = child;
            process::signal_kill(child.id());
            let _ = child.wait();
            child
        }
        #[cfg(windows)]
        {
            let mut child = Command::new("cmd")
                .args(["/C", "exit", "0"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn fixture child");
            let _ = child.wait();
            child
        }
    }

    // ── launching a fake browser: the real spawn path, no real browser ─────

    #[cfg(unix)]
    #[test]
    fn a_launch_that_never_becomes_ready_kills_the_child_and_prepares_the_profile() {
        let scratch = Scratch::new("launch_timeout");
        let binary = scratch.path("fake-browser");
        let pid_file = scratch.path("fake-browser.pid");
        write_executable(&binary);
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\necho $$ > {}\nsleep 60\n",
                pid_file.display()
            ),
        )
        .expect("write fake browser");
        let workspace = scratch.path("workspace");
        let app_data = scratch.path("app-data");
        std::fs::create_dir_all(&workspace).expect("workspace");

        let error = start_binary(
            &binary,
            "Fake",
            &workspace,
            &app_data,
            Duration::from_millis(400),
        )
        .expect_err("a browser that never answers is a failure");

        assert!(
            matches!(error, BrowserError::NotReady { .. }),
            "expected NotReady, got {error:?}"
        );
        assert!(
            app_data.join(PROFILE_DIR_NAME).is_dir(),
            "the dedicated profile is created before the spawn"
        );

        let pid_text = std::fs::read_to_string(&pid_file).expect("the fake browser recorded its pid");
        let pid: u32 = pid_text.trim().parse().expect("a pid");
        let mut waited = Duration::from_millis(0);
        while process_alive(pid) && waited < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(25));
            waited += Duration::from_millis(25);
        }
        assert!(!process_alive(pid), "the child must be killed, not leaked");
    }
}
