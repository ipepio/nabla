// MCP configuration through pi's own command line (F0 of
// `odd/tasks/tool-drawer-and-mcp.md`, S1).
//
// Design notes:
// - Nabla owns no MCP schema. `mcp.json` (`~/.pi/agent/mcp.json` global,
//   `.pi/mcp.json` project) belongs to pi, its OAuth tokens live in
//   `~/.pi/agent/mcp-auth.json`, and every read or write here is a `pi mcp`
//   invocation. Nothing in this module opens those files.
// - argv only: the builders below answer a typed argument vector, never a
//   string handed to a shell, so a server name, an env value or a url can never
//   be read as shell syntax. Same rule as the CLI bridge (`cli.rs`).
// - the exit code is not evidence of state (L of the feature doc: `pi mcp login`
//   of an unknown server exits 0). State comes from `pi mcp list --json`.
// - the parser is defensive on purpose: it reads today's captured shape and
//   tolerates missing fields, an object-shaped `transport` and unknown `state`
//   words, because the payload is pi's to change. It never panics.
//
// The DTOs serialize camelCase so the TS contracts mirror them field by field,
// like every other payload that crosses this seam.

use std::path::{Path, PathBuf};

/// Longest server name accepted, in bytes. pi keys a server by its name, and a
/// name is compared literally across the global and project files.
pub const NAME_MAX_LEN: usize = 64;

/// Deadline of `pi mcp list --json`. It is not the generic 20 s because listing
/// is not a file read: pi connects to every enabled server to learn its state
/// and its tools, so a slow HTTP server is part of a normal answer.
pub const LIST_TIMEOUT_MS: u64 = 60_000;

/// Global file pi reads, relative to the user's home: `$HOME/.pi/agent/mcp.json`.
/// It is pi's default agent directory, which is the one a Nabla child resolves,
/// because the bridge forwards `HOME` and not `PI_CODING_AGENT_DIR` (see
/// `process::FORWARDED_ENV_EXACT`). A shell started elsewhere may point pi at a
/// different agent directory; that is not the file a session spawned here reads.
pub const GLOBAL_MCP_FILE: &str = ".pi/agent/mcp.json";

/// Project file pi reads, relative to the project root (a trusted project).
pub const PROJECT_MCP_FILE: &str = ".pi/mcp.json";

/// Which of pi's two files a call addresses (S1). Global is pi's default, so
/// only the project scope adds a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpScope {
    Global,
    Project,
}

impl McpScope {
    /// Reads the scope pi reports. An unknown word is read as `global`, which is
    /// pi's own default and never hides a project file behind a wrong badge.
    pub fn parse(raw: &str) -> McpScope {
        match raw.trim().to_ascii_lowercase().as_str() {
            "project" | "local" => McpScope::Project,
            _ => McpScope::Global,
        }
    }

    /// The CLI flag selecting `.pi/mcp.json`: `-l`. Absent for the global file
    /// because that is the default.
    pub fn flag(self) -> Option<&'static str> {
        match self {
            McpScope::Global => None,
            McpScope::Project => Some("-l"),
        }
    }
}

/// How pi reaches a server. pi's JSON names no discriminant: `transport` is the
/// url for an HTTP server and the command line for a stdio one, so the kind is
/// read from the target itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Stdio,
    Http,
}

/// Badge state of one server (S1). `NeedsSignIn` is the state the UI turns into
/// "Sign in"; `Unknown` is what an unrecognized word becomes instead of a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpState {
    Ready,
    NeedsSignIn,
    Error,
    Disabled,
    Unknown,
}

impl McpState {
    /// The words pi uses for each state, compared case- and separator-insensitively
    /// (`needs-auth`, `needs_auth` and `Needs Auth` are one word here).
    /// `None` means "unrecognized", which the caller reports as `unknown`.
    pub fn parse(raw: &str) -> Option<McpState> {
        let normalized = raw.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        match normalized.as_str() {
            "connected" | "ready" | "ok" => Some(McpState::Ready),
            "needs_auth" | "needs_authentication" | "needs_sign_in" | "needs_signin"
            | "login_required" | "unauthorized" => Some(McpState::NeedsSignIn),
            "failed" | "error" | "disconnected" | "closed" => Some(McpState::Error),
            "disabled" | "off" => Some(McpState::Disabled),
            "unknown" | "connecting" => Some(McpState::Unknown),
            _ => None,
        }
    }
}

/// One server as the pane paints it (S1): where it comes from, how pi reaches
/// it, whether pi could connect and which tools it exposes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerSummary {
    pub name: String,
    pub scope: McpScope,
    pub transport: McpTransport,
    /// The command line of a stdio server or the url of an HTTP one, verbatim as
    /// pi reported it: the UI shows the human what is going to run.
    pub target: String,
    pub enabled: bool,
    pub state: McpState,
    /// Tool names the server exposed. Empty unless the server is connected.
    pub tools: Vec<String>,
    /// pi's own diagnostic, present only for a failed connection.
    pub error: Option<String>,
}

/// One `KEY=VALUE` pair of a draft: an environment variable of a stdio server or
/// a header of an HTTP one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpKeyValue {
    pub key: String,
    pub value: String,
}

/// What the add form fills in (S1). One flat shape for both transports: the
/// fields of the unused transport are ignored, so the form can keep its state
/// while the human switches between stdio and HTTP. Every optional field
/// defaults, so the webview sends only what the human typed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAddDraft {
    pub name: String,
    pub scope: McpScope,
    pub transport: McpTransport,
    /// stdio: the executable to run.
    #[serde(default)]
    pub command: Option<String>,
    /// stdio: its arguments, one argv entry each.
    #[serde(default)]
    pub args: Vec<String>,
    /// stdio: environment variables (`--env KEY=VALUE`).
    #[serde(default)]
    pub env: Vec<McpKeyValue>,
    /// http: the streamable HTTP url.
    #[serde(default)]
    pub url: Option<String>,
    /// http: headers (`--header KEY=VALUE`), which may carry `${ENV}` or
    /// `!command` instead of a secret.
    #[serde(default)]
    pub headers: Vec<McpKeyValue>,
    /// http: send `Authorization: Bearer ${NAME}` instead of storing a token
    /// (`--bearer-token-env-var NAME`).
    #[serde(default)]
    pub bearer_token_env_var: Option<String>,
    /// How the model reaches this server's tools (`--exposure`).
    ///
    /// Absent leaves pi's default, `codemode`, which declares **nothing** to the
    /// model: those tools only exist inside codemode scripts, and the agent sees
    /// no `mcp__<server>__<tool>` at all. A caller that wants callable tools has
    /// to say `direct`, which is what the app's own server needs.
    #[serde(default)]
    pub exposure: Option<String>,
}

/// Why a draft or a call was refused. Typed and testable inside Rust; the
/// command layer maps it to a plain string for the webview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpError {
    /// The server name is empty, too long or outside `[A-Za-z0-9._-]`.
    InvalidName(String),
    /// A stdio server without a command.
    MissingCommand,
    /// An HTTP server without a url.
    MissingUrl,
    /// A url that is not `http://` or `https://`.
    InvalidUrl(String),
    /// An env key that is empty, or carries `=` or whitespace.
    InvalidEnvKey(String),
    /// A header name that is empty, or carries `=` or whitespace.
    InvalidHeaderKey(String),
    /// `pi mcp login --timeout 0`: pi would give up before opening a browser.
    InvalidTimeout(u64),
    /// An `--exposure` value outside the modes pi accepts.
    InvalidExposure(String),
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpError::InvalidName(name) => write!(
                f,
                "mcp: invalid server name {name:?}: 1-{NAME_MAX_LEN} characters from [A-Za-z0-9._-]"
            ),
            McpError::MissingCommand => write!(
                f,
                "mcp: a stdio server needs a command (pi mcp add <server> -- <command> [args...])"
            ),
            McpError::MissingUrl => write!(
                f,
                "mcp: an http server needs a url (pi mcp add <server> --url <url>)"
            ),
            McpError::InvalidUrl(url) => write!(f, "mcp: not an http(s) url: {url:?}"),
            McpError::InvalidEnvKey(key) => write!(
                f,
                "mcp: invalid env key {key:?}: it must be non-empty and carry no '=' or whitespace"
            ),
            McpError::InvalidHeaderKey(key) => write!(
                f,
                "mcp: invalid header name {key:?}: it must be non-empty and carry no '=' or whitespace"
            ),
            McpError::InvalidTimeout(seconds) => write!(
                f,
                "mcp: the login timeout must be at least 1 second, got {seconds}"
            ),
            McpError::InvalidExposure(mode) => {
                write!(f, "mcp: unknown exposure {mode:?}: use one of ")?;
                for (index, expected) in EXPOSURE_MODES.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{expected}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for McpError {}

/// `pi mcp list --json`. The only source of MCP state: never a file read, and
/// never the exit code (L of the feature doc).
pub fn list_args() -> Vec<String> {
    vec![
        "mcp".to_string(),
        "list".to_string(),
        "--json".to_string(),
    ]
}

/// Parses the JSON of `pi mcp list --json`. Never panics: anything it cannot
/// understand answers an empty list (or an entry with `state: unknown`), because
/// a listing that breaks must not break the pane.
///
/// The captured shape is `{ "servers": [ ... ], "errors": [ ... ] }`. An entry
/// needs a non-blank `name` to be addressable, so a nameless one is dropped
/// instead of becoming a row nothing can act on; every other field is optional.
/// A bare array of entries is accepted too, which is the only reasonable reading
/// of a payload whose envelope changed.
pub fn parse_list(json: &str) -> Vec<McpServerSummary> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let entries: &[serde_json::Value] = match &value {
        serde_json::Value::Array(items) => items.as_slice(),
        serde_json::Value::Object(map) => match map.get("servers") {
            Some(serde_json::Value::Array(items)) => items.as_slice(),
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    entries.iter().filter_map(summary_of).collect()
}

/// One entry of the listing. `None` only for an entry that is not an object or
/// has no usable name: everything else is reported, with `unknown` for a state
/// this code does not recognize.
fn summary_of(entry: &serde_json::Value) -> Option<McpServerSummary> {
    let map = entry.as_object()?;
    let name = map.get("name").and_then(|value| value.as_str())?.trim();
    if name.is_empty() {
        return None;
    }
    let scope = map
        .get("scope")
        .and_then(|value| value.as_str())
        .map(McpScope::parse)
        .unwrap_or(McpScope::Global);
    // pi writes `enabled: config.enabled !== false`, so an absent field is an
    // enabled server, not a disabled one.
    let enabled = map
        .get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let (transport, target) = transport_of(map.get("transport"));
    // A disabled entry is never probed by pi, so its state is `disabled` whatever
    // the payload says about a connection that did not happen.
    let state = if !enabled {
        McpState::Disabled
    } else {
        map.get("state")
            .and_then(|value| value.as_str())
            .and_then(McpState::parse)
            .unwrap_or(McpState::Unknown)
    };
    let error = map
        .get("error")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_string);
    Some(McpServerSummary {
        name: name.to_string(),
        scope,
        transport,
        target,
        enabled,
        state,
        tools: tool_names(map.get("tools")),
        error,
    })
}

/// Reads the transport of one entry and the target that names it.
///
/// pi's payload has no discriminant: `transport` is the url for an HTTP server
/// and the command line for a stdio one, so the kind comes from the target. The
/// object form is tolerated because it is the shape a different pi version could
/// emit, and a wrong reading here would only mislabel a badge.
fn transport_of(value: Option<&serde_json::Value>) -> (McpTransport, String) {
    match value {
        Some(serde_json::Value::String(raw)) => {
            let target = raw.trim().to_string();
            let transport = if is_http_url(&target) {
                McpTransport::Http
            } else {
                McpTransport::Stdio
            };
            (transport, target)
        }
        Some(serde_json::Value::Object(map)) => {
            if let Some(url) = map.get("url").and_then(|value| value.as_str()) {
                return (McpTransport::Http, url.trim().to_string());
            }
            if let Some(command) = map.get("command").and_then(|value| value.as_str()) {
                let mut parts = vec![command.trim().to_string()];
                if let Some(serde_json::Value::Array(args)) = map.get("args") {
                    parts.extend(
                        args.iter()
                            .filter_map(|arg| arg.as_str())
                            .map(str::to_string),
                    );
                }
                return (McpTransport::Stdio, parts.join(" "));
            }
            let declared = map
                .get("type")
                .or_else(|| map.get("kind"))
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let transport = if declared == "http" || declared == "sse" {
                McpTransport::Http
            } else {
                McpTransport::Stdio
            };
            (transport, String::new())
        }
        _ => (McpTransport::Stdio, String::new()),
    }
}

/// Tool names of one entry. pi reports `[String]`; an object with a `name` is
/// read too, and anything else in the array is skipped instead of failing the
/// entry.
fn tool_names(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(serde_json::Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            serde_json::Value::String(name) => Some(name.trim()),
            serde_json::Value::Object(map) => {
                map.get("name").and_then(|value| value.as_str()).map(str::trim)
            }
            _ => None,
        })
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// Exact argv of an add: `pi mcp add <server> [-l] [--env K=V ...] -- <command>
/// [args...]` for stdio, `pi mcp add <server> [-l] --url <url> [--header K=V ...]
/// [--bearer-token-env-var NAME]` for HTTP.
///
/// The transport decides which fields are read, so the form may keep the fields
/// of the other transport without sending them. Nothing is interpolated: every
/// value is its own argv entry.
pub fn add_args(draft: &McpAddDraft) -> Result<Vec<String>, McpError> {
    validate_name(&draft.name)?;
    let mut args = vec![
        "mcp".to_string(),
        "add".to_string(),
        draft.name.trim().to_string(),
    ];
    if let Some(flag) = draft.scope.flag() {
        args.push(flag.to_string());
    }
    // Before the transport: `--exposure` is a flag of `pi mcp add`, not of the
    // server, so it must precede the `--` separator a stdio server uses.
    if let Some(mode) = draft
        .exposure
        .as_deref()
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
    {
        args.push("--exposure".to_string());
        args.push(validate_exposure(mode)?.to_string());
    }
    match draft.transport {
        McpTransport::Stdio => {
            let command = draft
                .command
                .as_deref()
                .map(str::trim)
                .filter(|command| !command.is_empty())
                .ok_or(McpError::MissingCommand)?;
            for pair in &draft.env {
                args.push("--env".to_string());
                args.push(format!("{}={}", validate_key(&pair.key, true)?, pair.value));
            }
            // The `--` separator ends pi's own options, so the command and its
            // arguments cannot be read as flags of `pi mcp add`.
            args.push("--".to_string());
            args.push(command.to_string());
            args.extend(draft.args.iter().cloned());
        }
        McpTransport::Http => {
            let url = draft
                .url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .ok_or(McpError::MissingUrl)?;
            if !is_http_url(url) {
                return Err(McpError::InvalidUrl(url.to_string()));
            }
            args.push("--url".to_string());
            args.push(url.to_string());
            for pair in &draft.headers {
                args.push("--header".to_string());
                args.push(format!("{}={}", validate_key(&pair.key, false)?, pair.value));
            }
            if let Some(name) = draft
                .bearer_token_env_var
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                args.push("--bearer-token-env-var".to_string());
                args.push(name.to_string());
            }
        }
    }
    Ok(args)
}

/// Exact argv of a remove: `pi mcp remove <server> [-l]`.
pub fn remove_args(name: &str, scope: McpScope) -> Result<Vec<String>, McpError> {
    scoped_args("remove", name, scope)
}

/// Exact argv of a sign-in: `pi mcp login <server> [-l] [--timeout <seconds>]`.
/// `timeout` absent means pi's own default (300 s).
pub fn login_args(
    name: &str,
    scope: McpScope,
    timeout_secs: Option<u64>,
) -> Result<Vec<String>, McpError> {
    let mut args = scoped_args("login", name, scope)?;
    if let Some(seconds) = timeout_secs {
        // A zero deadline would give up before the browser could open.
        if seconds == 0 {
            return Err(McpError::InvalidTimeout(seconds));
        }
        args.push("--timeout".to_string());
        args.push(seconds.to_string());
    }
    Ok(args)
}

/// Exact argv of a sign-out: `pi mcp logout <server> [-l]`. It deletes the stored
/// OAuth credential of that server; Nabla never touches `mcp-auth.json`.
pub fn logout_args(name: &str, scope: McpScope) -> Result<Vec<String>, McpError> {
    scoped_args("logout", name, scope)
}

/// Shared shape of `pi mcp <verb> <server> [-l]`, so the three verbs that only
/// name a server can never drift apart.
fn scoped_args(verb: &str, name: &str, scope: McpScope) -> Result<Vec<String>, McpError> {
    validate_name(name)?;
    let mut args = vec![
        "mcp".to_string(),
        verb.to_string(),
        name.trim().to_string(),
    ];
    if let Some(flag) = scope.flag() {
        args.push(flag.to_string());
    }
    Ok(args)
}

/// A `KEY=VALUE` key: non-empty, with no `=` and no whitespace, so pi reads the
/// pair as one env variable or one header.
fn validate_key(key: &str, env: bool) -> Result<String, McpError> {
    let key = key.trim();
    let usable = !key.is_empty() && !key.contains('=') && !key.chars().any(char::is_whitespace);
    if usable {
        return Ok(key.to_string());
    }
    Err(if env {
        McpError::InvalidEnvKey(key.to_string())
    } else {
        McpError::InvalidHeaderKey(key.to_string())
    })
}

/// Whether a string is an `http://` or `https://` url with no whitespace inside.
fn is_http_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let scheme_ok = lower.starts_with("http://") || lower.starts_with("https://");
    let rest = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or_default();
    scheme_ok && !rest.is_empty() && !url.chars().any(char::is_whitespace)
}

/// The absolute file a scope addresses. Nothing here creates a directory or a
/// file: pi writes it on the first `add`, and the path is only for the "open the
/// file" and "show me the JSON" actions.
pub fn file_path(home: &Path, project: &Path, scope: McpScope) -> PathBuf {
    match scope {
        McpScope::Global => home.join(GLOBAL_MCP_FILE),
        McpScope::Project => project.join(PROJECT_MCP_FILE),
    }
}

/// The modes `pi mcp add --exposure` accepts.
///
/// `codemode` is pi's default and declares **nothing** to the model: the tools
/// only exist inside codemode scripts, so an agent that was promised
/// `mcp__<server>__<tool>` finds no such tool. `direct` is the one that makes them
/// ordinary tools, and the reason this list is not passed through blindly.
pub const EXPOSURE_MODES: [&str; 5] = [
    "direct",
    "codemode",
    "codemode-deferred",
    "deferred",
    "hidden",
];

fn validate_exposure(mode: &str) -> Result<&str, McpError> {
    match EXPOSURE_MODES.iter().find(|known| **known == mode) {
        Some(_) => Ok(mode),
        None => Err(McpError::InvalidExposure(mode.to_string())),
    }
}

/// Rejects a server name that is empty, longer than `NAME_MAX_LEN` or outside
/// `[A-Za-z0-9._-]`. The argv builders never interpolate a shell, so this is not
/// an escaping rule: it keeps a name addressable in `pi mcp remove <name>` and
/// readable in the badge.
pub fn validate_name(name: &str) -> Result<(), McpError> {
    let name = name.trim();
    let safe = !name.is_empty()
        && name.len() <= NAME_MAX_LEN
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if safe {
        Ok(())
    } else {
        Err(McpError::InvalidName(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape captured from a real `pi mcp list --json` on this machine: one
    /// global stdio server named `engram`, connected, with its tools. Kept
    /// verbatim apart from the tool list being shortened, so a change in pi's
    /// output shows up here as a parse failure instead of a silent misread.
    const LIST_READY: &str = r#"{
      "servers": [
        {
          "name": "engram",
          "scope": "global",
          "source": "/Users/somebody/.pi/agent/mcp.json",
          "enabled": true,
          "exposure": "codemode",
          "transport": "/opt/homebrew/bin/engram mcp --tools=agent",
          "state": "connected",
          "tools": ["mem_save", "mem_search", "mem_update"]
        }
      ],
      "errors": []
    }"#;

    /// An HTTP server pi knows but cannot use yet, in a trusted project.
    const LIST_NEEDS_AUTH: &str = r#"{
      "servers": [
        {
          "name": "linear",
          "scope": "project",
          "source": ".pi/mcp.json",
          "enabled": true,
          "exposure": "codemode",
          "transport": "https://mcp.linear.app/sse",
          "state": "needs-auth",
          "tools": []
        }
      ],
      "errors": []
    }"#;

    /// A server that failed to connect: pi reports the state and its diagnostic.
    const LIST_ERROR: &str = r#"{
      "servers": [
        {
          "name": "broken",
          "scope": "global",
          "source": "/Users/somebody/.pi/agent/mcp.json",
          "enabled": true,
          "exposure": "deferred",
          "transport": "npx -y broken-mcp",
          "state": "failed",
          "tools": [],
          "error": "connect ECONNREFUSED 127.0.0.1:9999"
        }
      ],
      "errors": []
    }"#;

    /// Disabled in `mcp.json`: pi reports it without connecting.
    const LIST_DISABLED: &str = r#"{
      "servers": [
        {
          "name": "off",
          "scope": "global",
          "source": "/Users/somebody/.pi/agent/mcp.json",
          "enabled": false,
          "exposure": "codemode",
          "transport": "npx -y off-mcp",
          "state": "disabled",
          "tools": []
        }
      ],
      "errors": []
    }"#;

    fn one(json: &str) -> McpServerSummary {
        let mut servers = parse_list(json);
        assert_eq!(servers.len(), 1, "expected exactly one server: {servers:#?}");
        servers.remove(0)
    }

    fn draft(transport: McpTransport, scope: McpScope) -> McpAddDraft {
        McpAddDraft {
            name: "engram".to_string(),
            scope,
            transport,
            command: None,
            args: Vec::new(),
            env: Vec::new(),
            url: None,
            headers: Vec::new(),
            bearer_token_env_var: None,
            exposure: None,
        }
    }

    fn pair(key: &str, value: &str) -> McpKeyValue {
        McpKeyValue {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    // ── parse (S1) ──────────────────────────────────────────────────────────

    // S1: a connected stdio server keeps its command line, its tools and its scope.
    #[test]
    fn parse_list_reads_a_connected_stdio_server() {
        let server = one(LIST_READY);
        assert_eq!(server.name, "engram");
        assert_eq!(server.scope, McpScope::Global);
        assert_eq!(server.transport, McpTransport::Stdio);
        assert_eq!(server.target, "/opt/homebrew/bin/engram mcp --tools=agent");
        assert!(server.enabled);
        assert_eq!(server.state, McpState::Ready);
        assert_eq!(
            server.tools,
            strings(&["mem_save", "mem_search", "mem_update"])
        );
        assert_eq!(server.error, None, "a connected server reports no error");
    }

    // S1: an HTTP server that needs a sign-in is the badge the UI turns into
    // "Sign in", and its url is its target.
    #[test]
    fn parse_list_maps_an_http_server_that_needs_sign_in() {
        let server = one(LIST_NEEDS_AUTH);
        assert_eq!(server.name, "linear");
        assert_eq!(server.scope, McpScope::Project);
        assert_eq!(server.transport, McpTransport::Http);
        assert_eq!(server.target, "https://mcp.linear.app/sse");
        assert_eq!(server.state, McpState::NeedsSignIn);
        assert!(server.tools.is_empty());
    }

    // S1: a failed connection carries pi's diagnostic, so the pane can explain it.
    #[test]
    fn parse_list_reports_a_failed_server_and_its_error() {
        let server = one(LIST_ERROR);
        assert_eq!(server.state, McpState::Error);
        assert_eq!(
            server.error.as_deref(),
            Some("connect ECONNREFUSED 127.0.0.1:9999")
        );
        assert_eq!(server.transport, McpTransport::Stdio);
    }

    // S1: a disabled entry is `disabled`, never a probe result.
    #[test]
    fn parse_list_marks_a_disabled_server() {
        let server = one(LIST_DISABLED);
        assert!(!server.enabled);
        assert_eq!(server.state, McpState::Disabled);
        assert!(server.tools.is_empty());
    }

    // S1: unknown fields, an object-shaped transport, a missing state and a bare
    // entry are all read without panicking, and an entry with no name is dropped
    // because nothing can address it.
    #[test]
    fn parse_list_tolerates_unknown_fields_and_shapes() {
        let json = r#"{
          "servers": [
            {
              "name": "object-transport",
              "transport": { "type": "http", "url": "https://example.test/mcp" },
              "state": "some-future-word"
            },
            { "name": "bare" },
            { "name": "stdio-object", "transport": { "type": "stdio", "command": "npx", "args": ["-y", "srv"] } },
            { "scope": "global" },
            "not an object"
          ],
          "errors": ["config error: bad entry"]
        }"#;
        let servers = parse_list(json);
        assert_eq!(servers.len(), 3, "the nameless entries are dropped: {servers:#?}");

        assert_eq!(servers[0].name, "object-transport");
        assert_eq!(servers[0].transport, McpTransport::Http);
        assert_eq!(servers[0].target, "https://example.test/mcp");
        assert_eq!(servers[0].state, McpState::Unknown);
        assert_eq!(servers[0].scope, McpScope::Global, "an absent scope is global");
        assert!(servers[0].enabled, "an absent enabled flag is enabled");

        assert_eq!(servers[1].name, "bare");
        assert_eq!(servers[1].transport, McpTransport::Stdio);
        assert_eq!(servers[1].target, "");
        assert_eq!(servers[1].state, McpState::Unknown);

        assert_eq!(servers[2].transport, McpTransport::Stdio);
        assert_eq!(servers[2].target, "npx -y srv");
    }

    // S1: the tool list is a list of names; an object-shaped tool and a blank
    // name are tolerated instead of breaking the parse.
    #[test]
    fn parse_list_reads_tool_names_from_either_shape() {
        let json = r#"{
          "servers": [
            {
              "name": "tools",
              "transport": "npx -y srv",
              "state": "connected",
              "tools": ["alpha", { "name": "beta" }, "", 42]
            }
          ]
        }"#;
        assert_eq!(one(json).tools, strings(&["alpha", "beta"]));
    }

    // S1: an empty list and garbage are both "no servers", never a panic and
    // never an invented entry.
    #[test]
    fn parse_list_answers_empty_for_empty_lists_and_garbage() {
        for raw in [
            "",
            "not json",
            "null",
            "42",
            "\"a string\"",
            "{}",
            r#"{"servers":",]"#,
            r#"{"servers":[],"errors":[]}"#,
            "[]",
            r#"{"servers":{}}"#,
        ] {
            assert!(
                parse_list(raw).is_empty(),
                "expected an empty list for {raw:?}"
            );
        }
    }

    // Contract shape: one flat object, camelCase fields, `snake_case` state words
    // (the TS mirror reads them field by field).
    #[test]
    fn mcp_server_summary_serializes_with_the_contract_field_names() {
        let server = McpServerSummary {
            name: "linear".to_string(),
            scope: McpScope::Project,
            transport: McpTransport::Http,
            target: "https://mcp.linear.app/sse".to_string(),
            enabled: true,
            state: McpState::NeedsSignIn,
            tools: Vec::new(),
            error: None,
        };
        assert_eq!(
            serde_json::to_value(&server).expect("serialize"),
            serde_json::json!({
                "name": "linear",
                "scope": "project",
                "transport": "http",
                "target": "https://mcp.linear.app/sse",
                "enabled": true,
                "state": "needs_sign_in",
                "tools": [],
                "error": null
            })
        );
    }

    // The scope words of the contract, and the flag each one adds.
    #[test]
    fn scope_parse_and_flag_follow_the_documented_words() {
        assert_eq!(McpScope::parse("global"), McpScope::Global);
        assert_eq!(McpScope::parse("project"), McpScope::Project);
        assert_eq!(McpScope::parse("local"), McpScope::Project);
        assert_eq!(McpScope::parse(""), McpScope::Global);
        assert_eq!(McpScope::Global.flag(), None);
        assert_eq!(McpScope::Project.flag(), Some("-l"));
    }

    // ── argv builders (S1) ──────────────────────────────────────────────────

    // S1: stdio is `pi mcp add <server> [opts] -- <command> [args...]`.
    #[test]
    fn add_args_builds_the_documented_stdio_shape() {
        let mut stdio = draft(McpTransport::Stdio, McpScope::Global);
        stdio.command = Some("/opt/homebrew/bin/engram".to_string());
        stdio.args = strings(&["mcp", "--tools=agent"]);
        stdio.env = vec![pair("ENGRAM_LOG", "debug"), pair("TOKEN", "abc=def")];

        assert_eq!(
            add_args(&stdio).expect("a valid stdio draft"),
            strings(&[
                "mcp",
                "add",
                "engram",
                "--env",
                "ENGRAM_LOG=debug",
                "--env",
                "TOKEN=abc=def",
                "--",
                "/opt/homebrew/bin/engram",
                "mcp",
                "--tools=agent",
            ])
        );
    }

    // S1: HTTP is `pi mcp add <server> [opts] --url <url>` with repeatable
    // headers and the bearer-token env var.
    #[test]
    fn add_args_builds_the_documented_http_shape() {
        let mut http = draft(McpTransport::Http, McpScope::Global);
        http.url = Some("https://mcp.example.test/mcp".to_string());
        http.headers = vec![pair("X-Tenant", "acme"), pair("Authorization", "${ACME_TOKEN}")];
        http.bearer_token_env_var = Some("MCP_TOKEN".to_string());
        // Fields of the other transport are ignored, so the form may keep them.
        http.command = Some("ignored".to_string());

        assert_eq!(
            add_args(&http).expect("a valid http draft"),
            strings(&[
                "mcp",
                "add",
                "engram",
                "--url",
                "https://mcp.example.test/mcp",
                "--header",
                "X-Tenant=acme",
                "--header",
                "Authorization=${ACME_TOKEN}",
                "--bearer-token-env-var",
                "MCP_TOKEN",
            ])
        );
    }

    // S1: the project scope is the `-l` flag; the global file is the default.
    #[test]
    fn add_args_adds_the_local_flag_for_the_project_scope() {
        let mut project = draft(McpTransport::Stdio, McpScope::Project);
        project.command = Some("npx".to_string());
        project.args = strings(&["-y", "some-mcp"]);

        assert_eq!(
            add_args(&project).expect("a valid project draft"),
            strings(&["mcp", "add", "engram", "-l", "--", "npx", "-y", "some-mcp"])
        );
    }

    // S1: an empty optional field is simply absent, never an empty flag value.
    #[test]
    fn add_args_omits_empty_optional_fields() {
        let mut http = draft(McpTransport::Http, McpScope::Global);
        http.url = Some("https://mcp.example.test/mcp".to_string());
        http.bearer_token_env_var = Some("   ".to_string());

        assert_eq!(
            add_args(&http).expect("a valid http draft"),
            strings(&["mcp", "add", "engram", "--url", "https://mcp.example.test/mcp"])
        );
    }

    // S6 of `webview-control.md`: the mode pi uses to decide whether a tool is
    // declared to the model at all. It goes before the `--` separator, because it
    // is a flag of `pi mcp add` and not of the server's own argv, and a mode pi
    // does not know is refused here instead of writing a broken registration.
    #[test]
    fn add_args_places_a_known_exposure_before_the_command_separator() {
        let mut stdio = draft(McpTransport::Stdio, McpScope::Global);
        stdio.command = Some("npx".to_string());
        stdio.args = vec!["-y".to_string(), "some-mcp".to_string()];
        stdio.exposure = Some("direct".to_string());
        assert_eq!(
            add_args(&stdio).expect("a valid stdio draft"),
            strings(&[
                "mcp", "add", "engram", "--exposure", "direct", "--", "npx", "-y", "some-mcp",
            ])
        );

        // A blank mode means "say nothing", which leaves pi's default in place.
        let mut blank = draft(McpTransport::Http, McpScope::Global);
        blank.url = Some("https://mcp.example.test/mcp".to_string());
        blank.exposure = Some("  ".to_string());
        assert_eq!(
            add_args(&blank).expect("a blank exposure is not an error"),
            strings(&["mcp", "add", "engram", "--url", "https://mcp.example.test/mcp"])
        );

        // A mode pi does not accept never reaches its command line.
        let mut unknown = draft(McpTransport::Http, McpScope::Global);
        unknown.url = Some("https://mcp.example.test/mcp".to_string());
        unknown.exposure = Some("everywhere".to_string());
        assert_eq!(
            add_args(&unknown),
            Err(McpError::InvalidExposure("everywhere".to_string()))
        );
    }

    // S1: what the add form may not send is refused before anything is spawned.
    #[test]
    fn add_args_rejects_invalid_drafts() {
        // A name outside `[A-Za-z0-9._-]` could not be addressed by a later
        // `pi mcp remove <name>`.
        let mut named = draft(McpTransport::Stdio, McpScope::Global);
        named.name = "bad name".to_string();
        named.command = Some("npx".to_string());
        assert_eq!(
            add_args(&named),
            Err(McpError::InvalidName("bad name".to_string()))
        );

        // stdio needs a command, and a blank one is not a command.
        let mut empty_command = draft(McpTransport::Stdio, McpScope::Global);
        assert_eq!(add_args(&empty_command), Err(McpError::MissingCommand));
        empty_command.command = Some("   ".to_string());
        assert_eq!(add_args(&empty_command), Err(McpError::MissingCommand));

        // http needs an http(s) url.
        let mut no_url = draft(McpTransport::Http, McpScope::Global);
        assert_eq!(add_args(&no_url), Err(McpError::MissingUrl));
        no_url.url = Some("ftp://example.test/mcp".to_string());
        assert_eq!(
            add_args(&no_url),
            Err(McpError::InvalidUrl("ftp://example.test/mcp".to_string()))
        );
        no_url.url = Some("https://example.test/mcp".to_string());
        assert!(add_args(&no_url).is_ok(), "a valid https url is accepted");

        // env keys and header names must be usable as `KEY=VALUE`.
        let mut bad_env = draft(McpTransport::Stdio, McpScope::Global);
        bad_env.command = Some("npx".to_string());
        bad_env.env = vec![pair("A B", "1")];
        assert_eq!(
            add_args(&bad_env),
            Err(McpError::InvalidEnvKey("A B".to_string()))
        );
        bad_env.env = vec![pair("KEY=VALUE", "1")];
        assert_eq!(
            add_args(&bad_env),
            Err(McpError::InvalidEnvKey("KEY=VALUE".to_string()))
        );
        bad_env.env = vec![pair("", "1")];
        assert_eq!(
            add_args(&bad_env),
            Err(McpError::InvalidEnvKey(String::new()))
        );

        let mut bad_header = draft(McpTransport::Http, McpScope::Global);
        bad_header.url = Some("https://example.test/mcp".to_string());
        bad_header.headers = vec![pair("", "v")];
        assert_eq!(
            add_args(&bad_header),
            Err(McpError::InvalidHeaderKey(String::new()))
        );
    }

    // S1: remove, login and logout are `pi mcp <verb> <server> [-l]`, with
    // `--timeout <seconds>` only when a deadline is asked for.
    #[test]
    fn remove_login_and_logout_args_match_the_documented_shapes() {
        assert_eq!(
            remove_args("engram", McpScope::Global).expect("a valid name"),
            strings(&["mcp", "remove", "engram"])
        );
        assert_eq!(
            remove_args("engram", McpScope::Project).expect("a valid name"),
            strings(&["mcp", "remove", "engram", "-l"])
        );
        assert_eq!(
            login_args("linear", McpScope::Global, None).expect("a valid name"),
            strings(&["mcp", "login", "linear"])
        );
        assert_eq!(
            login_args("linear", McpScope::Project, Some(120)).expect("a valid name"),
            strings(&["mcp", "login", "linear", "-l", "--timeout", "120"])
        );
        assert_eq!(
            logout_args("linear", McpScope::Project).expect("a valid name"),
            strings(&["mcp", "logout", "linear", "-l"])
        );
    }

    // S1: a sign-in with no time left to open a browser is refused here instead
    // of failing after a spawn.
    #[test]
    fn login_args_rejects_a_zero_timeout() {
        assert_eq!(
            login_args("linear", McpScope::Global, Some(0)),
            Err(McpError::InvalidTimeout(0))
        );
        assert_eq!(
            remove_args("", McpScope::Global),
            Err(McpError::InvalidName(String::new()))
        );
    }

    // S1: the listing is `pi mcp list --json`, one argv entry per token.
    #[test]
    fn list_args_ask_for_json() {
        assert_eq!(list_args(), strings(&["mcp", "list", "--json"]));
    }

    // ── files (S1) ──────────────────────────────────────────────────────────

    // S1: each scope names pi's own file, absolutely.
    #[test]
    fn file_path_names_pi_files_for_each_scope() {
        let home = Path::new("/home/somebody");
        let project = Path::new("/work/project");

        assert_eq!(
            file_path(home, project, McpScope::Global),
            Path::new("/home/somebody/.pi/agent/mcp.json")
        );
        assert_eq!(
            file_path(home, project, McpScope::Project),
            Path::new("/work/project/.pi/mcp.json")
        );
        assert!(
            file_path(home, project, McpScope::Project).is_absolute(),
            "the pane shows and opens an absolute path"
        );
    }

    // S1: a name is addressable when it is 1-64 characters of `[A-Za-z0-9._-]`.
    #[test]
    fn validate_name_accepts_shell_safe_names_only() {
        for good in ["engram", "chrome-devtools_mcp", "a.b", "A1"] {
            assert_eq!(validate_name(good), Ok(()), "expected {good:?} to be valid");
        }
        for bad in ["", "  ", "with space", "semi;colon", "quote'", "back\\slash", "../up"] {
            assert!(validate_name(bad).is_err(), "expected {bad:?} to be invalid");
        }
        assert!(validate_name(&"a".repeat(NAME_MAX_LEN)).is_ok());
        assert!(validate_name(&"a".repeat(NAME_MAX_LEN + 1)).is_err());
    }
}
