//! The MCP **server**: the bridge that lets pi drive the embedded webview.
//!
//! The webview already has its commands (`embed_*`), and `mcp.rs` already covers
//! pi's *client* side — adding, listing and logging into MCP servers. What was
//! missing is the other end: something pi can connect to in order to reach the
//! view the human is looking at (`odd/tasks/webview-control.md`, S1-S2).
//!
//! The shape is deliberately boring. A local HTTP endpoint speaks MCP's
//! *streamable HTTP* binding over JSON-RPC 2.0, exposes the tools a browser agent
//! needs, and each tool calls the same driver the UI's commands call. No new
//! protocol, no second browser: the agent drives the page the human already sees.
//!
//! ```text
//! pi ──POST /mcp (JSON-RPC)──▶ this module ──WebviewDriver──▶ embed::EmbedSessions
//! ```
//!
//! Security is the whole reason this file is careful (S6). The endpoint executes
//! JavaScript in a view that navigates with the human's cookies, so it binds
//! `127.0.0.1` only, on an ephemeral port, and every request must carry the token
//! minted for this run — in the `Authorization` header or the `token` query, so a
//! URL-only registration is still possible. Without it any local process would
//! inherit that capability.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{json, Value};
use tauri::Emitter;

use crate::embed::EmbedError;

/// The name pi will see in `tools/list` sources and in its own tool names
/// (`mcp__nabla-browser__navigate`).
pub const SERVER_NAME: &str = "nabla-browser";

/// How long a tool that needs a view waits for the app to open the pane
/// (`auto-open-pane.md`, S4). Bounded like `REGISTRATION_WAIT`: the pane may never
/// appear, and a tool call must not hang on that.
pub const VIEW_WAIT: Duration = Duration::from_secs(10);

/// The protocol revision this server speaks. It echoes the client's request when
/// it recognises it, and falls back to this one otherwise.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// One live child webview, as a tool sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveView {
    pub id: String,
    pub url: String,
}

/// The seam between the MCP tools and whatever actually drives a webview.
///
/// A trait, not a direct call, because the dispatcher is the part worth testing:
/// it decides which view a tool acts on, what a tool failure looks like on the
/// wire, and what an unknown method answers. None of those need a webview — or
/// even a window — to be verified.
pub trait WebviewDriver: Send + Sync + 'static {
    /// Every open view, in a stable order.
    fn views(&self) -> Vec<LiveView>;

    /// Asks the app to open the browser pane and waits at most `timeout` for the
    /// view to exist (`auto-open-pane.md`, S1/S3/S4). `true` means a view is there.
    ///
    /// Required, not defaulted: a driver that cannot open a pane must say so,
    /// otherwise a tool would report "no view" without ever having asked.
    fn ensure_view(&self, timeout: Duration) -> bool;

    /// Navigates one existing view. The url is validated by the driver, which is
    /// where the `http`/`https`/`about:blank` rule already lives.
    fn navigate(&self, view_id: &str, url: &str) -> Result<(), DriverFailure>;

    /// Runs a script in one existing view and answers what the platform
    /// serialized from it.
    fn eval(&self, view_id: &str, script: &str) -> Result<String, DriverFailure>;
}

/// A driver failure, typed so the dispatcher can tell an accident from an answer.
///
/// Flattening both into one string is what used to let a platform detail reach the
/// model as if the app were broken. The split is the dispatcher's whole retry rule:
/// [`DriverFailure::ViewGone`] is the one failure worth one more attempt, because
/// the human may have closed the pane between resolving the view and using it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverFailure {
    /// The view the tool resolved is no longer driveable: the registry lost it, or
    /// its webview did. The driver has already evicted the dead entry.
    ViewGone,
    /// Everything the driver actually answered — a script error, a refused url, a
    /// page that never replied. Rendered to the model verbatim.
    Other(String),
}

impl DriverFailure {
    /// A failure that is an answer, not an accident: nothing to retry.
    pub fn other(message: impl Into<String>) -> DriverFailure {
        DriverFailure::Other(message.into())
    }
}

impl std::fmt::Display for DriverFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Replaced by the honest no-view message before it reaches the model;
            // readable here so a test or a log line still says what happened.
            DriverFailure::ViewGone => write!(f, "the browser view is no longer open"),
            DriverFailure::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for DriverFailure {}

/// The endpoint one run of the app offers, and the token that opens it.
///
/// T3 of `webview-control.md` writes the url into pi's `mcp.json`; until then this
/// is what the app knows about its own server.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    /// `http://127.0.0.1:<port>/mcp` — the ephemeral port of this run.
    pub url: String,
    /// The bearer token every request must carry.
    pub token: String,
}

// ── the dispatcher: JSON-RPC in, JSON-RPC out ───────────────────────────────

/// Answers one JSON-RPC message. `None` means the message was a notification and
/// MCP expects no body back.
///
/// The MCP rules this follows are the ones a client actually exercises:
/// `initialize` for the handshake, `tools/list` for discovery, `tools/call` for
/// work, and errors as JSON-RPC codes. Tool *failures* are different from
/// protocol errors on purpose: a bad selector is a normal outcome the model must
/// read, so it travels as `isError`, never as a transport failure.
pub fn dispatch(driver: &dyn WebviewDriver, request: &Value) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(Value::Null);

    // A notification has no id and never gets an answer, not even an error.
    let id = id?;

    let result = match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            Some(json!({
                "protocolVersion": asked,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
            }))
        }
        "ping" => Some(json!({})),
        "tools/list" => Some(json!({ "tools": tool_catalog() })),
        "tools/call" => return Some(call_tool(driver, &id, &params)),
        _ => None,
    };

    match result {
        Some(result) => Some(json!({ "jsonrpc": "2.0", "id": id, "result": result })),
        None => Some(error(&id, -32601, &format!("unknown method `{method}`"))),
    }
}

/// `tools/call`: resolve the view, run the tool, and turn its outcome into
/// content the model reads — including failures.
///
/// Every tool but `list_targets` acts on one view, so resolution happens once,
/// before the tool runs. When no view exists AND the caller did not name one, the
/// nine that need a view ask the app to open the pane (`auto-open-pane.md`,
/// S1/S3) and use what the UI creates with its own rect. A caller that names a
/// `viewId` is not asking for a pane: it already said which view it means, so an
/// id that is not open is answered by name, immediately (see [`resolve_view`]).
/// `list_targets` is the tool whose *answer* is the registry: an empty list is a
/// valid reply, not a failure, so it never asks for a pane.
///
/// A tool that hits a view which disappeared under it (the human closed the pane
/// between resolving and using it, or a registry entry outlived its webview) asks
/// for a pane once and retries once — see the block below the resolution, and
/// [`reopen`] for why a NAMED view never triggers that ask. A view that goes away
/// twice, or a pane nobody opens, answers [`NO_VIEW_ERROR`]: the platform's own
/// words would read to the model as a broken app.
fn call_tool(driver: &dyn WebviewDriver, id: &Value, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if !TOOL_NAMES.contains(&name) {
        return error(id, -32602, &format!("unknown tool `{name}`"));
    }
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    let asked_view = arguments.get("viewId").and_then(Value::as_str);

    let mut views = driver.views();
    if name == "list_targets" {
        return tool_result(id, &targets_text(&views), false);
    }

    // Only the caller that did NOT name a view is asking for one. Naming an id
    // means "this view, the one I already know about", so an empty registry is
    // answered by `resolve_view` instead: a diagnostic `get_url` with an explicit
    // `viewId` used to open the human's drawer and hold the call for the whole
    // `VIEW_WAIT`, which is the pane competing with the call that only wanted to
    // read a url.
    if asked_view.is_none() && views.is_empty() {
        // The ask is bounded, so a pane nobody opens costs a short wait, never a
        // hung call. A false answer is not a failure of the app but a state of it:
        // the human closed the pane, or the Browser tool is off in Settings (S5 —
        // nothing is enabled behind the human's back).
        if !driver.ensure_view(VIEW_WAIT) {
            return tool_result(id, NO_VIEW_ERROR, true);
        }
        views = driver.views();
    }

    let view = match resolve_view(&views, asked_view) {
        Ok(view) => view.clone(),
        Err(message) => return tool_result(id, &message, true),
    };

    let action = match Action::parse(name, &arguments) {
        Ok(action) => action,
        Err(message) => return error(id, -32602, &message),
    };

    // One retry, never a loop. `ViewGone` means the view this call resolved is
    // already gone (the human closed the pane in between, or an entry outlived its
    // webview) and the driver has evicted the dead entry. Asking for a pane once
    // and retrying once is what turns "embed: no embedded browser embed-1 is open"
    // — which reads to the model as "the app is broken" — into either the work it
    // asked for or the honest no-view answer. Everything else is an answer: a
    // script error, a refused url, a selector that matches nothing.
    let outcome = match action.run(driver, &view) {
        Err(DriverFailure::ViewGone) => match reopen(driver, asked_view) {
            Ok(view) => action.run(driver, &view),
            Err(message) => Err(DriverFailure::other(message)),
        },
        outcome => outcome,
    };

    match outcome {
        Ok(text) => tool_result(id, &text, false),
        // A view that went away twice is the same honest answer as a pane nobody
        // opened: the tool needs a view and there is none to drive. The driver's
        // own words here are an app-level detail the model cannot act on.
        Err(DriverFailure::ViewGone) => tool_result(id, NO_VIEW_ERROR, true),
        Err(failure) => tool_result(id, &failure.to_string(), true),
    }
}

/// Asks the app for a pane once and resolves the view to retry on.
///
/// The `Err` is a message already fit for the model: either the honest no-view
/// answer, or what `resolve_view` refuses.
///
/// A named view is never a request for a pane here either, for the same reason it
/// is not one above: the caller said which view it means, so a named id that is
/// gone is re-resolved against the registry as it is now and answered by name —
/// not replaced with whatever the app would open next. That keeps the second
/// `ensure_view` in this file obeying the first one's rule.
fn reopen(driver: &dyn WebviewDriver, asked: Option<&str>) -> Result<LiveView, String> {
    if asked.is_none() && !driver.ensure_view(VIEW_WAIT) {
        return Err(NO_VIEW_ERROR.to_string());
    }
    let views = driver.views();
    resolve_view(&views, asked).cloned()
}

/// What one tool run needs, resolved before the driver is called.
///
/// The arguments a tool cannot run without are checked in [`Action::parse`], so a
/// missing one stays a protocol error (a broken request) and can never be mistaken
/// for a page that failed. Splitting the action out is also what lets the retry
/// re-run the same tool against a new view without a second copy of this match.
enum Action {
    Navigate(String),
    Eval(String),
    GetUrl,
    GetTitle,
    Click(String),
    Type { selector: String, text: String },
    Back,
    Forward,
    Reload,
}

impl Action {
    /// The action a view-needing tool name means, or the message its arguments
    /// earn. Every name here is in [`TOOL_NAMES`] and is not `list_targets`.
    fn parse(name: &str, arguments: &Value) -> Result<Action, String> {
        fn needed<'a>(arguments: &'a Value, key: &str, tool: &str) -> Result<&'a str, String> {
            argument(arguments, key).ok_or_else(|| format!("{tool} needs a `{key}` argument"))
        }
        Ok(match name {
            "navigate" => Action::Navigate(needed(arguments, "url", name)?.to_string()),
            "eval" => Action::Eval(needed(arguments, "script", name)?.to_string()),
            "get_url" => Action::GetUrl,
            "get_title" => Action::GetTitle,
            "click" => Action::Click(needed(arguments, "selector", name)?.to_string()),
            "type" => {
                let selector = needed(arguments, "selector", name)?.to_string();
                // An empty text is a request to clear the field, which is a real
                // thing to do, so only a missing or non-string one is refused.
                let Some(text) = arguments.get("text").and_then(Value::as_str) else {
                    return Err("type needs a `text` argument".to_string());
                };
                Action::Type {
                    selector,
                    text: text.to_string(),
                }
            }
            "back" => Action::Back,
            "forward" => Action::Forward,
            "reload" => Action::Reload,
            other => unreachable!("`{other}` is in TOOL_NAMES and has no arm"),
        })
    }

    /// Runs against one view. `Err` is a driver failure, so the caller can tell the
    /// one retryable case from the answers.
    fn run(&self, driver: &dyn WebviewDriver, view: &LiveView) -> Result<String, DriverFailure> {
        match self {
            Action::Navigate(url) => driver
                .navigate(&view.id, url)
                .map(|()| format!("navigated `{}` to {url}", view.id)),
            Action::Eval(script) => driver.eval(&view.id, script),
            // The url the registry holds for this view: the agent's own navigation
            // is not the only way it changes, so the page is never asked. The
            // registry is re-read rather than trusted from the snapshot this call
            // resolved: an entry evicted in between would otherwise answer a url
            // for a view nobody has, which is the stale url this tool must never
            // report.
            Action::GetUrl => {
                if driver.views().iter().any(|live| live.id == view.id) {
                    Ok(view.url.clone())
                } else {
                    Err(DriverFailure::ViewGone)
                }
            }
            Action::GetTitle => driver.eval(&view.id, TITLE_SCRIPT),
            Action::Click(selector) => run_script(driver, view, &click_script(selector)),
            Action::Type { selector, text } => {
                run_script(driver, view, &type_script(selector, text))
            }
            Action::Back => history(driver, view, "history.back()", "went back one history entry"),
            Action::Forward => history(
                driver,
                view,
                "history.forward()",
                "went forward one history entry",
            ),
            Action::Reload => history(driver, view, "location.reload()", "reloaded the page"),
        }
    }
}

/// A non-empty string argument, or `None`. An empty one is treated as absent:
/// it would reach the page as a selector that matches nothing.
fn argument<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

/// Runs one generated script and keeps its answer.
///
/// A page that cannot satisfy the request says so *inside* the script, as
/// `{"error": "..."}`, instead of throwing: `eval_with_callback` ignores an
/// exception (Tauri documents the limitation), so a throw would cost the whole
/// timeout and deliver no message. The failure is unpacked here, where it becomes
/// an `isError` result the model can read and retry.
fn run_script(
    driver: &dyn WebviewDriver,
    view: &LiveView,
    script: &str,
) -> Result<String, DriverFailure> {
    let answer = driver.eval(&view.id, script)?;
    match serde_json::from_str::<Value>(&answer) {
        Ok(value) => match value.get("error").and_then(Value::as_str) {
            Some(message) => Err(DriverFailure::other(message)),
            None => Ok(answer),
        },
        Err(_) => Ok(answer),
    }
}

/// One of the three history moves, which answer their own confirmation: the
/// platform serializes `history.back()` as `null`, which tells the model nothing.
fn history(
    driver: &dyn WebviewDriver,
    view: &LiveView,
    script: &str,
    done: &str,
) -> Result<String, DriverFailure> {
    driver.eval(&view.id, script).map(|_| done.to_string())
}

/// The page's own title, read where the page is.
const TITLE_SCRIPT: &str = "document.title";

/// A model-supplied string as a JavaScript string literal.
///
/// `serde_json`'s string syntax is a subset of JavaScript's, so its output is a
/// literal that cannot be closed early: a selector or a text holding `"`, `\\` or
/// a newline is escaped, never spliced in raw. This is the one place where the
/// model's text becomes code, so it is the one place that must not concatenate.
fn json_literal(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

/// Clicks the first element matching `selector`, after bringing it into view.
///
/// Scrolling first is not politeness: a programmatic click on an element nobody
/// can see is where a "it worked" answer hides a page that never moved.
fn click_script(selector: &str) -> String {
    format!(
        r#"(() => {{
  const selector = {literal};
  const element = document.querySelector(selector);
  if (!element) {{
    return {{ error: "nothing matches the selector " + selector }};
  }}
  element.scrollIntoView({{ block: "center", inline: "center" }});
  element.click();
  return {{ clicked: true }};
}})()"#,
        literal = json_literal(selector)
    )
}

/// Types `text` into the first element matching `selector`.
///
/// The value goes through the **native setter**: assigning `element.value`
/// updates the DOM and leaves React's and Vue's own state behind, so the next
/// render puts the old value back. Dispatching `input` and `change`, both
/// bubbling, is what makes the framework's listener see the edit. Focus comes
/// first because a field that is not focused does not look typed in either.
fn type_script(selector: &str, text: &str) -> String {
    format!(
        r#"(() => {{
  const selector = {selector};
  const text = {text};
  const element = document.querySelector(selector);
  if (!element) {{
    return {{ error: "nothing matches the selector " + selector }};
  }}
  const prototype = element instanceof HTMLTextAreaElement
    ? HTMLTextAreaElement.prototype
    : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(prototype, "value")?.set;
  if (!setter) {{
    return {{ error: "not a text field: " + selector }};
  }}
  element.focus();
  setter.call(element, text);
  element.dispatchEvent(new Event("input", {{ bubbles: true }}));
  element.dispatchEvent(new Event("change", {{ bubbles: true }}));
  return {{ typed: true }};
}})()"#,
        selector = json_literal(selector),
        text = json_literal(text)
    )
}

/// What a view-needing tool answers when no view exists, after asking for one:
/// the honest error of S4 of `auto-open-pane.md`.
///
/// Two causes are invisible from where the agent runs — the human closed the pane,
/// or the Browser tool is off in Settings — so the message names both and says
/// whose move it is.
const NO_VIEW_ERROR: &str = "no browser view is open and none appeared: the browser pane may be \
     closed, or the Browser tool may be disabled in Settings; ask the human to open the browser \
     pane or enable the Browser tool, then try again";

/// Which view a tool acts on: the named one, or the only one open.
///
/// The failures here are written for the model, not for a log: they say what to
/// do next, because "no view" and "three views" are recoverable by the agent and
/// by the human respectively.
fn resolve_view<'a>(views: &'a [LiveView], asked: Option<&str>) -> Result<&'a LiveView, String> {
    if let Some(asked) = asked {
        return views.iter().find(|view| view.id == asked).ok_or_else(|| {
            format!("no view with id `{asked}`; open now: {}", open_views(views))
        });
    }
    match views {
        [only] => Ok(only),
        // Reachable only when the app said a view exists and the registry no
        // longer holds one: the same honest answer as a wait that timed out.
        [] => Err(NO_VIEW_ERROR.to_string()),
        _ => Err(format!(
            "more than one view is open ({}); pass `viewId`",
            open_views(views)
        )),
    }
}

fn open_views(views: &[LiveView]) -> String {
    if views.is_empty() {
        return "none".to_string();
    }
    views
        .iter()
        .map(describe)
        .collect::<Vec<_>>()
        .join(", ")
}

/// One line per view, the shape `list_targets` and every "open now" message
/// share, so the agent reads the same thing wherever it looks.
fn describe(view: &LiveView) -> String {
    format!("{} ({})", view.id, view.url)
}

/// The registry as a tool answer: `id (url)` per line. An empty registry is an
/// answer, not an error — the tool's whole job is to report it.
fn targets_text(views: &[LiveView]) -> String {
    if views.is_empty() {
        return "no browser view is open; ask the human to open the browser pane first".to_string();
    }
    views.iter().map(describe).collect::<Vec<_>>().join("\n")
}

/// Every tool this server offers, in catalog order. Kept as data so a name that
/// is listed but not dispatched (or the reverse) fails a test instead of the
/// model's next call.
const TOOL_NAMES: [&str; 10] = [
    "navigate",
    "eval",
    "get_url",
    "get_title",
    "click",
    "type",
    "back",
    "forward",
    "reload",
    "list_targets",
];

/// The catalog. The schemas are what the model sees, so every field is
/// described: a bare `{"type":"object"}` makes the model guess.
///
/// `required` is declared on all ten and empty on the six tools that genuinely
/// demand nothing (`viewId` is the common case omitted), because a schema that
/// lies about its inputs is worse than one with an empty list.
fn tool_catalog() -> Vec<Value> {
    vec![
        json!({
            "name": "navigate",
            "description": "Open a url in the embedded browser view the human is looking at. \
                            Accepts http, https and about:blank only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The page to open." },
                    "viewId": {
                        "type": "string",
                        "description": "Which view, when more than one is open. Omit for the only one."
                    }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "eval",
            "description": "Run JavaScript in the embedded browser view and return its result as JSON. \
                            Use it to read the DOM, click, type or scroll.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "The script. Answer what you need: the last expression is the result."
                    },
                    "viewId": {
                        "type": "string",
                        "description": "Which view, when more than one is open. Omit for the only one."
                    }
                },
                "required": ["script"]
            }
        }),
        json!({
            "name": "get_url",
            "description": "The url the browser view is showing, as the app's own registry knows it. \
                            Costs no script in the page.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "viewId": {
                        "type": "string",
                        "description": "Which view, when more than one is open. Omit for the only one."
                    }
                },
                "required": []
            }
        }),
        json!({
            "name": "get_title",
            "description": "The page's own title (`document.title`) of the browser view.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "viewId": {
                        "type": "string",
                        "description": "Which view, when more than one is open. Omit for the only one."
                    }
                },
                "required": []
            }
        }),
        json!({
            "name": "click",
            "description": "Click the first element matching a CSS selector, scrolling it into view first. \
                            Fails with a message naming the selector when nothing matches — the page \
                            may still be loading, so retrying after a moment is reasonable.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector of the element to click."
                    },
                    "viewId": {
                        "type": "string",
                        "description": "Which view, when more than one is open. Omit for the only one."
                    }
                },
                "required": ["selector"]
            }
        }),
        json!({
            "name": "type",
            "description": "Type text into the first element matching a CSS selector, the way a human \
                            would: the value goes through the native setter and `input` and `change` \
                            fire, so controlled React and Vue inputs update. Use `eval` for anything else.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector of the field to type into."
                    },
                    "text": {
                        "type": "string",
                        "description": "What to type. An empty string clears the field."
                    },
                    "viewId": {
                        "type": "string",
                        "description": "Which view, when more than one is open. Omit for the only one."
                    }
                },
                "required": ["selector", "text"]
            }
        }),
        json!({
            "name": "back",
            "description": "Go back one entry in the history of the browser view.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "viewId": {
                        "type": "string",
                        "description": "Which view, when more than one is open. Omit for the only one."
                    }
                },
                "required": []
            }
        }),
        json!({
            "name": "forward",
            "description": "Go forward one entry in the history of the browser view.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "viewId": {
                        "type": "string",
                        "description": "Which view, when more than one is open. Omit for the only one."
                    }
                },
                "required": []
            }
        }),
        json!({
            "name": "reload",
            "description": "Reload the page in the browser view.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "viewId": {
                        "type": "string",
                        "description": "Which view, when more than one is open. Omit for the only one."
                    }
                },
                "required": []
            }
        }),
        json!({
            "name": "list_targets",
            "description": "List the open browser views, one `id (url)` per line. Use it when a call \
                            needs a `viewId`. An empty list is a normal answer: it means no view is \
                            open, and the human has to open the browser pane.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
    ]
}

/// A `tools/call` result. `isError` is how MCP reports a tool that ran but failed,
/// which is a different thing from a broken request.
fn tool_result(id: &Value, text: &str, is_error: bool) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": text }],
            "isError": is_error,
        }
    })
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

// ── the HTTP binding ───────────────────────────────────────────────────────

/// A token for this run: 32 random bytes, hex. `/dev/urandom`-grade entropy
/// through `getrandom`, with a clock fallback so a broken platform never leaves
/// the endpoint open without one.
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        bytes[..16].copy_from_slice(&nanos.to_le_bytes());
        bytes[16..24].copy_from_slice(&(std::process::id() as u128).to_le_bytes()[..8]);
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// ── the registration pi reads (S4 of `webview-control.md`) ─────────────────

/// The `pi mcp add` draft that puts this run's endpoint in front of every session
/// pi spawns (T3, S4).
///
/// Three decisions live here and nowhere else:
///
/// - **global scope**: the project scope writes `.pi/mcp.json` inside the human's
///   repository, and this entry belongs to the app, not to the project;
/// - **http, url of this run**: the port is ephemeral, so the entry is rewritten
///   on every start and removed on exit;
/// - **the token in a header** (S6): the url carries no secret, so a leaked
///   `mcp.json` or the output of `pi mcp list` never shows the key that opens a
///   view holding the human's cookies.
///
/// Turning this into argv is `mcp::add_args`; this function owns only what the
/// entry *is*, so it can be tested without pi or a socket.
pub fn registration_draft(endpoint: &Endpoint) -> crate::mcp::McpAddDraft {
    crate::mcp::McpAddDraft {
        name: SERVER_NAME.to_string(),
        scope: crate::mcp::McpScope::Global,
        transport: crate::mcp::McpTransport::Http,
        command: None,
        args: Vec::new(),
        env: Vec::new(),
        url: Some(endpoint.url.clone()),
        headers: vec![crate::mcp::McpKeyValue {
            key: "Authorization".to_string(),
            value: format!("Bearer {}", endpoint.token),
        }],
        // `direct`, never pi's default `codemode`: with codemode the tools are
        // only reachable from a codemode script and the model is told about
        // none of them, so `mcp__nabla-browser__navigate` does not exist and the
        // agent answers "Tool not found" while the server sits connected.
        exposure: Some("direct".to_string()),
        bearer_token_env_var: None,
    }
}

/// What the boot-time registration of this run's endpoint into pi's `mcp.json`
/// came to (S4 of `webview-control.md`).
///
/// A value, not a `Result`: "no endpoint was bound" is a state of the app, not a
/// failure, and a session start has to read all three the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationOutcome {
    /// The `nabla-browser` entry was written: a session started now has the tools.
    Registered,
    /// No endpoint was bound, so there was nothing to write.
    Skipped,
    /// The attempt failed, or never finished. The reason is kept for the one line
    /// that says what it costs.
    Failed(String),
}

impl RegistrationOutcome {
    /// The line to log when this outcome is a failure, phrased with its cost: a
    /// session started now runs without the browser tools.
    pub fn failure_notice(&self) -> Option<String> {
        match self {
            RegistrationOutcome::Failed(reason) => Some(format!(
                "nabla: the MCP registration failed ({reason}): \
                 the agent will not have the browser tools in sessions started now"
            )),
            RegistrationOutcome::Registered | RegistrationOutcome::Skipped => None,
        }
    }
}

/// How long a session start waits for the registration before starting anyway.
///
/// Bounded on purpose: the browser pane and the app work without the agent
/// tools, so a registration that never reports costs a short pause, never a
/// session that cannot be created.
pub const REGISTRATION_WAIT: Duration = Duration::from_secs(5);

/// The reason `wait` answers when nobody completed the gate in time.
pub const REGISTRATION_TIMEOUT: &str = "the registration did not finish in time";

/// The one-shot latch that closes the window this defect came from
/// (`webview-control.md` L6, L8).
///
/// pi reads `mcp.json` **once**, when it starts, so the tools a session has are
/// decided by that single read. The registration runs on a background thread and
/// is a `remove` followed by an `add`, which is a moment with no entry at all: a
/// session spawned inside it inherits a config without `nabla-browser` and keeps
/// that config for its whole life, with no error anywhere — every call just
/// answers "tool not found". This gate is the fence around that moment. A
/// completion (success, skipped or failure) opens it for every waiter, now and
/// later.
///
/// A `Condvar`, not an async primitive: the waiters are pi spawns, and pi spawns
/// already run on worker threads. `wait` therefore never runs on an async one.
pub struct RegistrationGate {
    state: Mutex<GateState>,
    opened: Condvar,
    reported: AtomicBool,
}

#[derive(Default)]
struct GateState {
    outcome: Option<RegistrationOutcome>,
}

impl RegistrationGate {
    pub fn new() -> RegistrationGate {
        RegistrationGate {
            state: Mutex::new(GateState::default()),
            opened: Condvar::new(),
            reported: AtomicBool::new(false),
        }
    }

    /// Records the outcome and opens the gate for everyone waiting on it.
    ///
    /// Idempotent: the first completion wins. Sessions may already have read and
    /// acted on it, so a later one cannot rewrite what the app already promised.
    pub fn complete(&self, outcome: RegistrationOutcome) {
        let mut state = self.lock();
        if state.outcome.is_none() {
            state.outcome = Some(outcome);
            // Woken while the lock is still held: a waiter cannot miss this
            // notification, because it re-checks the outcome under the lock.
            self.opened.notify_all();
        }
    }

    /// The outcome, waiting for it at most `timeout`.
    ///
    /// Returns immediately when the gate is already completed, and answers
    /// `Failed(REGISTRATION_TIMEOUT)` when nobody completes it in time instead of
    /// hanging the caller forever.
    pub fn wait(&self, timeout: Duration) -> RegistrationOutcome {
        let started = Instant::now();
        let mut state = self.lock();
        loop {
            if let Some(outcome) = state.outcome.as_ref() {
                return outcome.clone();
            }
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                return RegistrationOutcome::Failed(REGISTRATION_TIMEOUT.to_string());
            }
            // Spurious wake-ups loop back here, so the elapsed time is what
            // bounds the wait, never a single `wait_timeout` call.
            let (next, _) = self
                .opened
                .wait_timeout(state, timeout - elapsed)
                .unwrap_or_else(|poison| poison.into_inner());
            state = next;
        }
    }

    /// The line to log for `outcome`, at most once for this gate.
    ///
    /// Every session start asks the gate the same question, and a failure has one
    /// consequence: one line naming it, not one per session. Returns `None` for a
    /// success, and for a failure already announced.
    pub fn announce(&self, outcome: &RegistrationOutcome) -> Option<String> {
        let notice = outcome.failure_notice()?;
        if self.reported.swap(true, Ordering::SeqCst) {
            return None;
        }
        Some(notice)
    }

    fn lock(&self) -> MutexGuard<'_, GateState> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

impl Default for RegistrationGate {
    fn default() -> RegistrationGate {
        RegistrationGate::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::sync::Mutex;

    /// A driver that records what it was asked and answers what the test set.
    ///
    /// Its registry is mutable for one reason: a scripted `ViewGone` must remove
    /// the entry it answered for, because that is what the real driver does before
    /// it reports the failure. A fake that kept the dead entry would let a retry
    /// hit the same view again and prove nothing.
    struct FakeDriver {
        views: Mutex<Vec<LiveView>>,
        navigated: Mutex<Vec<(String, String)>>,
        evaluated: Mutex<Vec<(String, String)>>,
        /// What successive `navigate` calls answer, in order; the last entry
        /// repeats, so a one-entry script means "every call answers this".
        navigate_answers: Mutex<VecDeque<Result<(), DriverFailure>>>,
        /// Same rule for `eval`.
        eval_answers: Mutex<VecDeque<Result<String, DriverFailure>>>,
        /// What the app registers when a tool asks for a pane. Empty is the
        /// human keeping the pane closed, or the Browser tool being off.
        opens: Vec<LiveView>,
        /// The views `opens` has registered so far.
        opened: Mutex<Vec<LiveView>>,
        /// Every `ensure_view` call, with the timeout it was asked for.
        ensured: Mutex<Vec<Duration>>,
    }

    /// The next scripted answer: entries are popped in order and the last one
    /// repeats, so a script of one means "always", and an empty script is the
    /// type's own fallback.
    fn next_answer<T: Clone>(answers: &Mutex<VecDeque<T>>, fallback: T) -> T {
        let mut answers = answers.lock().expect("the answer script");
        if answers.len() > 1 {
            answers.pop_front().expect("a queued answer")
        } else {
            answers.front().cloned().unwrap_or(fallback)
        }
    }

    impl FakeDriver {
        fn with_views(views: Vec<LiveView>) -> FakeDriver {
            FakeDriver {
                views: Mutex::new(views),
                navigated: Mutex::new(Vec::new()),
                evaluated: Mutex::new(Vec::new()),
                navigate_answers: Mutex::new(VecDeque::from([Ok(())])),
                eval_answers: Mutex::new(VecDeque::from([Ok("\"ok\"".to_string())])),
                opens: Vec::new(),
                opened: Mutex::new(Vec::new()),
                ensured: Mutex::new(Vec::new()),
            }
        }

        fn one() -> FakeDriver {
            FakeDriver::with_views(vec![LiveView {
                id: "embed-1".to_string(),
                url: "https://example.com/".to_string(),
            }])
        }

        /// No view open, and a UI that registers `opens` when it is asked.
        fn opening(opens: Vec<LiveView>) -> FakeDriver {
            FakeDriver {
                opens,
                ..FakeDriver::with_views(Vec::new())
            }
        }

        /// How many times a tool asked the app for a pane.
        fn asks(&self) -> usize {
            self.ensured.lock().unwrap().len()
        }

        /// What successive `eval` calls answer; the last one repeats.
        fn answer_eval(&self, answer: Result<String, DriverFailure>) {
            *self.eval_answers.lock().unwrap() = VecDeque::from([answer]);
        }

        /// What successive `navigate` calls answer; the last one repeats.
        fn answer_navigate(&self, answers: impl IntoIterator<Item = Result<(), DriverFailure>>) {
            *self.navigate_answers.lock().unwrap() = answers.into_iter().collect();
        }

        /// What the real driver does with a `ViewGone`: the dead entry leaves the
        /// registry before the failure is answered.
        fn evict(&self, view_id: &str) {
            self.views
                .lock()
                .unwrap()
                .retain(|view| view.id != view_id);
        }

        /// The views the app registers once a tool asks for a pane, on an open
        /// registry: the shape of "the human closed the pane and the tool asked for
        /// it again".
        fn with_pane(mut self, opens: Vec<LiveView>) -> FakeDriver {
            self.opens = opens;
            self
        }

        /// Every id the fake's own registry holds, dead entries included or not.
        fn registered(&self) -> Vec<String> {
            self.views
                .lock()
                .unwrap()
                .iter()
                .map(|view| view.id.clone())
                .collect()
        }
    }

    impl WebviewDriver for FakeDriver {
        fn views(&self) -> Vec<LiveView> {
            let mut views = self.views.lock().unwrap().clone();
            views.extend(self.opened.lock().unwrap().iter().cloned());
            views
        }

        fn ensure_view(&self, timeout: Duration) -> bool {
            self.ensured.lock().unwrap().push(timeout);
            if self.opens.is_empty() {
                return false;
            }
            self.opened
                .lock()
                .unwrap()
                .extend(self.opens.iter().cloned());
            true
        }

        fn navigate(&self, view_id: &str, url: &str) -> Result<(), DriverFailure> {
            self.navigated
                .lock()
                .unwrap()
                .push((view_id.to_string(), url.to_string()));
            let answer = next_answer(&self.navigate_answers, Ok(()));
            if answer == Err(DriverFailure::ViewGone) {
                self.evict(view_id);
            }
            answer
        }

        fn eval(&self, view_id: &str, script: &str) -> Result<String, DriverFailure> {
            self.evaluated
                .lock()
                .unwrap()
                .push((view_id.to_string(), script.to_string()));
            let answer = next_answer(
                &self.eval_answers,
                Err(DriverFailure::other("the fake has no answer left")),
            );
            if answer == Err(DriverFailure::ViewGone) {
                self.evict(view_id);
            }
            answer
        }
    }

    fn request(method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params })
    }

    // S2: the handshake a client performs before anything else. Answering the
    // client's own revision is what keeps pi's negotiation from downgrading.
    #[test]
    fn initialize_answers_the_handshake_with_tools() {
        let driver = FakeDriver::one();
        let answer = dispatch(
            &driver,
            &request(
                "initialize",
                json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "pi" } }),
            ),
        )
        .expect("initialize is a request, not a notification");

        assert_eq!(answer["id"], json!(7));
        assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(answer["result"]["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(answer["result"]["capabilities"]["tools"]["listChanged"], false);
    }

    // A notification never gets a body: `notifications/initialized` after the
    // handshake is where a server that answers anyway breaks the client.
    #[test]
    fn a_notification_gets_no_answer() {
        let driver = FakeDriver::one();
        let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert_eq!(dispatch(&driver, &notification), None);
    }

    // S1: the model discovers everything it can drive, with a schema it can fill.
    // Every tool declares `required`; it is empty exactly where the design gives
    // the tool no mandatory argument (the registry and the view's own history are
    // the whole input), and that emptiness is asserted, not skipped.
    #[test]
    fn tools_list_exposes_the_whole_catalog_with_schemas() {
        let driver = FakeDriver::one();
        let answer = dispatch(&driver, &request("tools/list", json!({}))).expect("answer");
        let tools = answer["result"]["tools"].as_array().expect("tools array");

        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, TOOL_NAMES.to_vec());
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
            assert!(
                tool["inputSchema"]["required"].as_array().is_some(),
                "{} must declare required, even when it is empty",
                tool["name"]
            );
        }

        let required = |name: &str| -> Value {
            let tool = tools
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap_or_else(|| panic!("{name} is not in the catalog"));
            tool["inputSchema"]["required"].clone()
        };
        assert_eq!(required("navigate"), json!(["url"]));
        assert_eq!(required("eval"), json!(["script"]));
        assert_eq!(required("click"), json!(["selector"]));
        assert_eq!(required("type"), json!(["selector", "text"]));

        // No mandatory argument, deliberately: `viewId` is the common case
        // omitted, and list_targets answers *with* the registry.
        for name in ["get_url", "get_title", "back", "forward", "reload", "list_targets"] {
            assert_eq!(required(name), json!([]), "{name} takes no mandatory argument");
        }
    }

    // S5: the tool reaches the view the human is looking at without the model
    // having to name it, which is the common case (one pane, one view).
    #[test]
    fn navigate_reaches_the_only_open_view() {
        let driver = FakeDriver::one();
        let answer = dispatch(
            &driver,
            &request(
                "tools/call",
                json!({ "name": "navigate", "arguments": { "url": "https://example.org/" } }),
            ),
        )
        .expect("answer");

        assert_eq!(answer["result"]["isError"], false);
        assert_eq!(
            driver.navigated.lock().unwrap().as_slice(),
            [("embed-1".to_string(), "https://example.org/".to_string())]
        );
    }

    // S5: `eval` hands back what the platform serialized, untouched — the model,
    // not this server, decides what the JSON means.
    #[test]
    fn eval_answers_the_script_result_verbatim() {
        let driver = FakeDriver::one();
        driver.answer_eval(Ok("{\"title\":\"Example Domain\"}".to_string()));
        let answer = dispatch(
            &driver,
            &request(
                "tools/call",
                json!({ "name": "eval", "arguments": { "script": "document.title" } }),
            ),
        )
        .expect("answer");

        assert_eq!(answer["result"]["isError"], false);
        assert_eq!(
            answer["result"]["content"][0]["text"],
            "{\"title\":\"Example Domain\"}"
        );
        assert_eq!(
            driver.evaluated.lock().unwrap().as_slice(),
            [("embed-1".to_string(), "document.title".to_string())]
        );
    }

    // The agent can recover from these by itself, so they are tool results with a
    // message that names the next step — never a bare protocol error.
    #[test]
    fn a_missing_or_ambiguous_view_is_a_tool_error_that_says_what_to_do() {
        let empty = FakeDriver::with_views(Vec::new());
        let no_view = dispatch(
            &empty,
            &request("tools/call", json!({ "name": "eval", "arguments": { "script": "1" } })),
        )
        .expect("answer");
        assert_eq!(no_view["result"]["isError"], true);
        assert!(
            no_view["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("open the browser pane"),
            "the message must tell the agent what the human has to do"
        );

        let two = FakeDriver::with_views(vec![
            LiveView { id: "embed-1".to_string(), url: "https://a.example/".to_string() },
            LiveView { id: "embed-2".to_string(), url: "https://b.example/".to_string() },
        ]);
        let ambiguous = dispatch(
            &two,
            &request("tools/call", json!({ "name": "eval", "arguments": { "script": "1" } })),
        )
        .expect("answer");
        assert_eq!(ambiguous["result"]["isError"], true);
        let text = ambiguous["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("viewId"), "the fix is naming one: {text}");
        assert!(text.contains("embed-1") && text.contains("embed-2"));

        // Naming one is enough, and the driver receives that id.
        let named = dispatch(
            &two,
            &request(
                "tools/call",
                json!({ "name": "eval", "arguments": { "script": "1", "viewId": "embed-2" } }),
            ),
        )
        .expect("answer");
        assert_eq!(named["result"]["isError"], false);
        assert_eq!(two.evaluated.lock().unwrap()[0].0, "embed-2");
    }

    // A tool that ran and failed is not a broken request: the model must see the
    // message and retry, so it travels as `isError`, with the protocol intact.
    #[test]
    fn a_failing_script_is_a_tool_error_not_a_protocol_error() {
        let driver = FakeDriver::one();
        driver.answer_eval(Err(DriverFailure::other("ReferenceError: foo is not defined")));
        let answer = dispatch(
            &driver,
            &request(
                "tools/call",
                json!({ "name": "eval", "arguments": { "script": "foo()" } }),
            ),
        )
        .expect("answer");

        assert!(answer["error"].is_null(), "a failed tool keeps the protocol clean");
        assert_eq!(answer["result"]["isError"], true);
        assert_eq!(
            answer["result"]["content"][0]["text"],
            "ReferenceError: foo is not defined"
        );
    }

    // Guessing costs a round trip: unknown names are refused with the JSON-RPC
    // codes a client understands, and no tool runs.
    #[test]
    fn unknown_method_and_unknown_tool_are_protocol_errors() {
        let driver = FakeDriver::one();
        let method = dispatch(&driver, &request("resources/list", json!({}))).expect("answer");
        assert_eq!(method["error"]["code"], -32601);

        let tool = dispatch(
            &driver,
            &request("tools/call", json!({ "name": "screenshot", "arguments": {} })),
        )
        .expect("answer");
        assert_eq!(tool["error"]["code"], -32602);
        assert!(driver.evaluated.lock().unwrap().is_empty());
    }

    // A tool call without its required argument is refused before it can drive
    // anything: an empty url would otherwise reach the webview as a no-op.
    #[test]
    fn a_tool_call_without_its_argument_is_refused() {
        let driver = FakeDriver::one();
        let answer = dispatch(
            &driver,
            &request("tools/call", json!({ "name": "navigate", "arguments": {} })),
        )
        .expect("answer");
        assert_eq!(answer["error"]["code"], -32602);
        assert!(driver.navigated.lock().unwrap().is_empty());

        // The DOM tools refuse the same way, before the page is touched.
        for (name, arguments) in [
            ("click", json!({})),
            ("type", json!({ "selector": "#email" })),
        ] {
            let answer = dispatch(
                &driver,
                &request("tools/call", json!({ "name": name, "arguments": arguments })),
            )
            .expect("answer");
            assert_eq!(answer["error"]["code"], -32602, "{name}");
        }
        assert!(
            driver.evaluated.lock().unwrap().is_empty(),
            "no page was touched"
        );
    }

    // S1: a selector and a text are the model's strings, and they reach an
    // evaluator. Embedded as a JSON string literal they cannot close the literal
    // early — quotes and backslashes are escaped, so a selector that looks like
    // the tail of a statement stays data.
    #[test]
    fn a_selector_and_a_text_are_embedded_as_json_string_literals() {
        let driver = FakeDriver::one();
        let selector = "li[data-note=\"a\"] .body\\x";
        let text = "say \"hi\" \\ bye";

        dispatch(
            &driver,
            &request(
                "tools/call",
                json!({ "name": "click", "arguments": { "selector": selector } }),
            ),
        )
        .expect("answer");
        dispatch(
            &driver,
            &request(
                "tools/call",
                json!({ "name": "type", "arguments": { "selector": selector, "text": text } }),
            ),
        )
        .expect("answer");

        let evaluated = driver.evaluated.lock().unwrap().clone();
        assert_eq!(evaluated.len(), 2);
        let (click_view, click_script) = &evaluated[0];
        let (type_view, type_script) = &evaluated[1];
        assert_eq!(click_view, "embed-1");
        assert_eq!(type_view, "embed-1");

        // The escaped form the evaluator receives, decoded again, is the string
        // the model sent — and the naive splice is not in the script.
        let escaped = serde_json::to_string(selector).expect("a JSON literal");
        for script in [click_script, type_script] {
            assert!(script.contains(&escaped), "escaped literal missing: {script}");
            assert_eq!(literal_after(script, "selector = "), selector, "{script}");
        }
        assert_eq!(literal_after(type_script, "text = "), text, "{type_script}");
    }

    /// The string literal a script embeds after `marker`, decoded.
    fn literal_after(script: &str, marker: &str) -> String {
        let tail = script
            .split_once(marker)
            .unwrap_or_else(|| panic!("no `{marker}` in {script}"))
            .1;
        let literal = tail
            .lines()
            .next()
            .expect("a line")
            .trim_end()
            .trim_end_matches(';');
        serde_json::from_str::<String>(literal)
            .unwrap_or_else(|error| panic!("`{literal}` is not one JSON string literal: {error}"))
    }

    // S1: click finds the element, brings it into view (a click on an offscreen
    // element is what fails silently) and clicks it.
    #[test]
    fn click_scrolls_the_element_into_view_then_clicks_it() {
        let driver = FakeDriver::one();
        driver.answer_eval(Ok("{\"clicked\":true}".to_string()));
        let answer = dispatch(
            &driver,
            &request(
                "tools/call",
                json!({ "name": "click", "arguments": { "selector": "#go" } }),
            ),
        )
        .expect("answer");

        assert_eq!(answer["result"]["isError"], false);
        assert_eq!(answer["result"]["content"][0]["text"], "{\"clicked\":true}");
        let script = driver.evaluated.lock().unwrap()[0].1.clone();
        for step in [
            "document.querySelector(selector)",
            "scrollIntoView",
            "element.click()",
        ] {
            assert!(script.contains(step), "{step} missing: {script}");
        }
    }

    // S1: a controlled input (React, Vue) only updates when the value goes
    // through the native setter and the events its framework listens for arrive.
    // Assigning `element.value` updates the DOM and leaves the framework behind.
    #[test]
    fn type_sets_the_value_through_the_native_setter_and_fires_the_events() {
        let driver = FakeDriver::one();
        driver.answer_eval(Ok("{\"typed\":true}".to_string()));
        let answer = dispatch(
            &driver,
            &request(
                "tools/call",
                json!({
                    "name": "type",
                    "arguments": { "selector": "#email", "text": "a@b.example" }
                }),
            ),
        )
        .expect("answer");

        assert_eq!(answer["result"]["isError"], false);
        assert_eq!(answer["result"]["content"][0]["text"], "{\"typed\":true}");
        let script = driver.evaluated.lock().unwrap()[0].1.clone();
        for step in [
            "Object.getOwnPropertyDescriptor",
            "setter.call(element, text)",
            "new Event(\"input\", { bubbles: true })",
            "new Event(\"change\", { bubbles: true })",
        ] {
            assert!(script.contains(step), "{step} missing: {script}");
        }
    }

    // A selector nobody matches is a normal outcome the model retries: the page
    // answers it inside the script (an exception never reaches us — Tauri ignores
    // it) and the tool reports a failure with the message intact.
    #[test]
    fn a_click_the_page_cannot_satisfy_is_a_tool_error_naming_the_selector() {
        let driver = FakeDriver::one();
        driver.answer_eval(Ok("{\"error\":\"nothing matches the selector #nope\"}".to_string()));
        let page = dispatch(
            &driver,
            &request(
                "tools/call",
                json!({ "name": "click", "arguments": { "selector": "#nope" } }),
            ),
        )
        .expect("answer");
        assert!(page["error"].is_null(), "a failed tool keeps the protocol clean");
        assert_eq!(page["result"]["isError"], true);
        assert_eq!(
            page["result"]["content"][0]["text"],
            "nothing matches the selector #nope"
        );

        // A driver failure of its own is preserved verbatim.
        let broken = FakeDriver::one();
        broken.answer_eval(Err(DriverFailure::other("embed-1 did not answer within 10s")));
        let answer = dispatch(
            &broken,
            &request(
                "tools/call",
                json!({ "name": "click", "arguments": { "selector": "#go" } }),
            ),
        )
        .expect("answer");
        assert_eq!(answer["result"]["isError"], true);
        assert_eq!(
            answer["result"]["content"][0]["text"],
            "embed-1 did not answer within 10s"
        );
    }

    // S5: two tools read the registry itself. A url or the list of targets costs
    // no round trip into the page — and must not run one.
    #[test]
    fn get_url_and_list_targets_answer_from_the_registry_without_a_script() {
        let driver = FakeDriver::with_views(vec![
            LiveView { id: "embed-1".to_string(), url: "https://a.example/".to_string() },
            LiveView {
                id: "embed-2".to_string(),
                url: "https://b.example/page".to_string(),
            },
        ]);

        let url = dispatch(
            &driver,
            &request(
                "tools/call",
                json!({ "name": "get_url", "arguments": { "viewId": "embed-2" } }),
            ),
        )
        .expect("answer");
        assert_eq!(url["result"]["isError"], false);
        assert_eq!(url["result"]["content"][0]["text"], "https://b.example/page");

        let targets = dispatch(
            &driver,
            &request("tools/call", json!({ "name": "list_targets", "arguments": {} })),
        )
        .expect("answer");
        assert_eq!(targets["result"]["isError"], false);
        assert_eq!(
            targets["result"]["content"][0]["text"],
            "embed-1 (https://a.example/)\nembed-2 (https://b.example/page)"
        );

        assert!(
            driver.evaluated.lock().unwrap().is_empty(),
            "the registry answers, not the page"
        );
    }

    // No view is the human's step, not a bug: the message says whose move it is.
    // And the server does not invent a view — only the UI knows where one goes,
    // so the honest answer is the existing one.
    #[test]
    fn get_url_on_an_empty_registry_asks_the_human_to_open_the_pane() {
        let driver = FakeDriver::with_views(Vec::new());
        let answer = dispatch(
            &driver,
            &request("tools/call", json!({ "name": "get_url", "arguments": {} })),
        )
        .expect("answer");

        assert_eq!(answer["result"]["isError"], true);
        let text = answer["result"]["content"][0]["text"].as_str().expect("text");
        assert!(text.contains("open the browser pane"), "{text}");
        assert!(driver.evaluated.lock().unwrap().is_empty());

        // An empty list of targets is an answer, not a failure.
        let targets = dispatch(
            &driver,
            &request("tools/call", json!({ "name": "list_targets", "arguments": {} })),
        )
        .expect("answer");
        assert_eq!(targets["result"]["isError"], false);
        assert!(
            targets["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("open the browser pane")
        );
    }

    // S1: the title is the page's own, read where the page is.
    #[test]
    fn get_title_reads_the_pages_own_title() {
        let driver = FakeDriver::one();
        driver.answer_eval(Ok("\"Example Domain\"".to_string()));
        let answer = dispatch(
            &driver,
            &request("tools/call", json!({ "name": "get_title", "arguments": {} })),
        )
        .expect("answer");

        assert_eq!(answer["result"]["isError"], false);
        assert_eq!(
            answer["result"]["content"][0]["text"],
            "\"Example Domain\""
        );
        assert_eq!(
            driver.evaluated.lock().unwrap().as_slice(),
            [("embed-1".to_string(), "document.title".to_string())]
        );
    }

    // S1: moving through the view's own history runs in the view the human is
    // looking at, and answers a confirmation flat enough to read.
    #[test]
    fn the_history_moves_and_reload_reach_the_only_open_view() {
        let driver = FakeDriver::one();
        for (name, message) in [
            ("back", "went back one history entry"),
            ("forward", "went forward one history entry"),
            ("reload", "reloaded the page"),
        ] {
            let answer = dispatch(
                &driver,
                &request("tools/call", json!({ "name": name, "arguments": {} })),
            )
            .expect("answer");
            assert_eq!(answer["result"]["isError"], false, "{name}");
            assert_eq!(answer["result"]["content"][0]["text"], message, "{name}");
        }

        assert_eq!(
            driver.evaluated.lock().unwrap().as_slice(),
            [
                ("embed-1".to_string(), "history.back()".to_string()),
                ("embed-1".to_string(), "history.forward()".to_string()),
                ("embed-1".to_string(), "location.reload()".to_string()),
            ]
        );
    }

    // ── opening the pane a tool needs (`auto-open-pane.md`) ─────────────────

    /// The nine tools that act on a view, with the least arguments they accept.
    const VIEW_TOOLS: [&str; 9] = [
        "navigate", "eval", "get_url", "get_title", "click", "type", "back", "forward",
        "reload",
    ];

    fn view_tool_arguments(name: &str) -> Value {
        match name {
            "navigate" => json!({ "url": "https://example.org/" }),
            "eval" => json!({ "script": "1" }),
            "click" => json!({ "selector": "#go" }),
            "type" => json!({ "selector": "#email", "text": "a@b.example" }),
            _ => json!({}),
        }
    }

    fn call(driver: &dyn WebviewDriver, name: &str) -> Value {
        dispatch(
            driver,
            &request(
                "tools/call",
                json!({ "name": name, "arguments": view_tool_arguments(name) }),
            ),
        )
        .expect("answer")
    }

    /// The same call, with the view NAMED — the case that must never open a pane.
    fn call_named(driver: &dyn WebviewDriver, name: &str, view_id: &str) -> Value {
        let mut arguments = view_tool_arguments(name);
        arguments["viewId"] = json!(view_id);
        dispatch(
            driver,
            &request("tools/call", json!({ "name": name, "arguments": arguments })),
        )
        .expect("answer")
    }

    fn answer_text(answer: &Value) -> &str {
        answer["result"]["content"][0]["text"]
            .as_str()
            .expect("a text result")
    }

    // S1/S3: no view open, so the tool asks the app for a pane and, once the UI
    // has created one with its real rect, drives THAT view. The geometry stays on
    // the UI side; here only the id comes back.
    #[test]
    fn a_tool_asks_the_app_for_a_pane_and_uses_the_view_it_opens() {
        let driver = FakeDriver::opening(vec![LiveView {
            id: "embed-7".to_string(),
            url: "about:blank".to_string(),
        }]);

        let answer = call(&driver, "navigate");

        assert_eq!(answer["result"]["isError"], false, "{}", answer_text(&answer));
        assert_eq!(
            *driver.ensured.lock().unwrap(),
            vec![VIEW_WAIT],
            "the tool waits with the shared bound"
        );
        assert_eq!(VIEW_WAIT, Duration::from_secs(10), "≈10s, like REGISTRATION_WAIT");
        assert_eq!(
            driver.navigated.lock().unwrap().as_slice(),
            [("embed-7".to_string(), "https://example.org/".to_string())],
            "the tool acts on the view the app just opened"
        );
    }

    // S4: no view appeared. The two causes are invisible from where the agent
    // runs, so the message names both and says whose move it is.
    #[test]
    fn a_tool_answers_an_honest_error_when_no_view_appears() {
        let driver = FakeDriver::opening(Vec::new());

        let answer = call(&driver, "eval");

        assert_eq!(answer["result"]["isError"], true);
        let text = answer_text(&answer);
        assert!(text.contains("browser pane"), "names the pane cause: {text}");
        assert!(text.contains("closed"), "names the closed pane: {text}");
        assert!(
            text.contains("disabled") && text.contains("Settings"),
            "names the switch that can turn the tool off: {text}"
        );
        assert!(text.contains("ask the human"), "says whose move it is: {text}");
        assert_eq!(driver.asks(), 1, "it asked once, then answered");
        assert!(driver.evaluated.lock().unwrap().is_empty(), "no page was touched");
        assert!(driver.navigated.lock().unwrap().is_empty());
    }

    // S1/S6: every view-needing tool asks, and only when the registry is empty.
    #[test]
    fn the_nine_view_tools_ask_for_a_pane_only_when_none_is_open() {
        for name in VIEW_TOOLS {
            let empty = FakeDriver::opening(Vec::new());
            let answer = call(&empty, name);
            assert_eq!(empty.asks(), 1, "{name} asks when no view is open");
            assert_eq!(answer["result"]["isError"], true, "{name}: no view, no work");

            let open = FakeDriver::one();
            let answer = call(&open, name);
            assert_eq!(open.asks(), 0, "{name} does not ask when a view is open");
            assert_eq!(
                answer["result"]["isError"], false,
                "{name}: {}",
                answer_text(&answer)
            );
        }
    }

    // S6: `list_targets` is the tool whose ANSWER is the registry. An empty list
    // stays a valid answer, so it never waits for a pane to appear.
    #[test]
    fn list_targets_never_asks_for_a_pane() {
        let driver = FakeDriver::opening(Vec::new());

        let answer = call(&driver, "list_targets");

        assert_eq!(answer["result"]["isError"], false, "an empty list is an answer");
        assert!(answer_text(&answer).contains("open the browser pane"));
        assert_eq!(driver.asks(), 0, "the registry answers; nothing waits");
    }

    // A caller that NAMED a view is not asking for one. The defect this pins:
    // `get_url` with an explicit `viewId` on an empty registry ran the auto-open
    // and waited the full `VIEW_WAIT`, so a diagnostic call moved the human's
    // drawer (and hung the caller). Naming an id answers that id's own honest
    // error, and the app is never asked to open anything.
    #[test]
    fn a_named_view_is_never_a_request_to_open_a_pane() {
        for name in VIEW_TOOLS {
            // The pane COULD be opened — the fake would register `embed-7` — which
            // is why the zero asks matter: nothing opens behind the caller's back.
            let driver = FakeDriver::opening(vec![LiveView {
                id: "embed-7".to_string(),
                url: "about:blank".to_string(),
            }]);

            let answer = call_named(&driver, name, "embed-1");

            assert_eq!(answer["result"]["isError"], true, "{name}");
            assert_eq!(driver.asks(), 0, "{name} must not ask for a pane");
            assert_eq!(
                answer_text(&answer),
                "no view with id `embed-1`; open now: none",
                "{name}: the named-missing answer, not the no-view one"
            );
            assert!(
                driver.views().is_empty(),
                "{name}: nothing was opened behind the caller: {:?}",
                driver.views()
            );
            assert!(driver.navigated.lock().unwrap().is_empty(), "{name}");
            assert!(driver.evaluated.lock().unwrap().is_empty(), "{name}");
        }
    }

    // The same rule inside one call: a NAMED view that disappeared under it is
    // answered with its own name, not with a pane the caller never asked for.
    #[test]
    fn a_named_view_that_disappeared_is_answered_by_name_not_by_a_new_pane() {
        let driver = FakeDriver::one().with_pane(vec![LiveView {
            id: "embed-7".to_string(),
            url: "about:blank".to_string(),
        }]);
        driver.answer_navigate([Err(DriverFailure::ViewGone)]);

        let answer = call_named(&driver, "navigate", "embed-1");

        assert_eq!(answer["result"]["isError"], true);
        assert_eq!(driver.asks(), 0, "no pane is opened for a named id");
        assert_eq!(
            answer_text(&answer),
            "no view with id `embed-1`; open now: none"
        );
        assert_eq!(
            driver.navigated.lock().unwrap().len(),
            1,
            "the one attempt on the named view, then the answer"
        );
    }

    // ── a view that disappears under a tool call ─────────────────────────────
    //
    // The defect: `list_targets` reported `embed-1` open, `navigate` answered the
    // platform's own words — "embed: no embedded browser embed-1 is open" — and the
    // model read a broken app instead of a closed pane. The driver now types that
    // failure, evicts the dead entry, and the dispatcher makes exactly one attempt
    // to get a view back.

    #[test]
    fn a_view_that_disappeared_is_retried_once_after_the_pane_reopens() {
        let driver = FakeDriver::one().with_pane(vec![LiveView {
            id: "embed-7".to_string(),
            url: "about:blank".to_string(),
        }]);
        driver.answer_navigate([Err(DriverFailure::ViewGone), Ok(())]);

        let answer = call(&driver, "navigate");

        assert_eq!(answer["result"]["isError"], false, "{}", answer_text(&answer));
        assert_eq!(answer_text(&answer), "navigated `embed-7` to https://example.org/");
        assert_eq!(driver.asks(), 1, "the tool asked the app for a pane exactly once");
        assert_eq!(
            driver.navigated.lock().unwrap().as_slice(),
            [
                ("embed-1".to_string(), "https://example.org/".to_string()),
                ("embed-7".to_string(), "https://example.org/".to_string()),
            ],
            "the dead view once, then the view the app opened"
        );
        // The dead entry left the registry: a second look no longer lists it, so
        // nothing can be resolved onto it again.
        assert!(
            driver.registered().is_empty(),
            "the driver evicted it: {:?}",
            driver.registered()
        );
        assert_eq!(
            driver.views().iter().map(|view| view.id.as_str()).collect::<Vec<_>>(),
            ["embed-7"]
        );
    }

    // Exactly one retry, never a loop: two `ViewGone` answers cost one ask and one
    // second attempt, and the model gets the honest no-view message — not the
    // platform's string.
    #[test]
    fn a_view_that_disappears_twice_gets_one_retry_then_the_honest_answer() {
        let driver = FakeDriver::one().with_pane(vec![LiveView {
            id: "embed-7".to_string(),
            url: "about:blank".to_string(),
        }]);
        driver.answer_navigate([Err(DriverFailure::ViewGone), Err(DriverFailure::ViewGone)]);

        let answer = call(&driver, "navigate");

        assert_eq!(answer["result"]["isError"], true);
        assert_eq!(answer_text(&answer), NO_VIEW_ERROR, "the honest answer, verbatim");
        assert_eq!(driver.asks(), 1, "one re-open ask, not one per attempt");
        assert_eq!(
            driver.navigated.lock().unwrap().len(),
            2,
            "the first attempt plus exactly one retry"
        );
        for leak in ["embed-", "no embedded browser", "could not navigate"] {
            assert!(
                !answer_text(&answer).contains(leak),
                "the platform's words must not reach the model: {leak}"
            );
        }
    }

    // A page failure is an answer: the script ran, the page said no, and asking the
    // human for a pane would be noise.
    #[test]
    fn a_script_error_is_an_answer_not_a_retry() {
        let driver = FakeDriver::one();
        driver.answer_eval(Err(DriverFailure::other("ReferenceError: foo is not defined")));

        let answer = call(&driver, "eval");

        assert_eq!(answer["result"]["isError"], true);
        assert_eq!(answer_text(&answer), "ReferenceError: foo is not defined");
        assert_eq!(driver.asks(), 0, "nothing was asked for a pane");
        assert_eq!(driver.evaluated.lock().unwrap().len(), 1, "the script ran once");
    }

    // `get_url` answers from the registry, so it is the one tool that can report a
    // url for a view the registry has already dropped. It re-reads the registry
    // instead of trusting the snapshot, and a stale entry is `ViewGone` — which the
    // dispatcher turns into the honest no-view answer, never a stale url.
    #[test]
    fn get_url_never_answers_the_url_of_an_evicted_view() {
        let driver = FakeDriver::with_views(Vec::new());
        let stale = LiveView {
            id: "embed-1".to_string(),
            url: "https://example.com/".to_string(),
        };

        assert_eq!(
            Action::GetUrl.run(&driver, &stale),
            Err(DriverFailure::ViewGone),
            "a snapshot the registry no longer holds is not an answer"
        );
        // And a live entry still answers with its own url, without a script.
        let live = FakeDriver::one();
        assert_eq!(
            Action::GetUrl.run(&live, &stale),
            Ok("https://example.com/".to_string())
        );
        assert!(live.evaluated.lock().unwrap().is_empty(), "the page was not asked");
    }

    // `list_targets` is the tool whose answer IS the registry, and an eviction
    // leaves it empty: an empty list is an answer, not a failure.
    #[test]
    fn list_targets_still_answers_an_empty_registry_after_an_eviction() {
        let driver = FakeDriver::one();
        driver.answer_navigate([Err(DriverFailure::ViewGone)]);

        let failed = call(&driver, "navigate");
        assert_eq!(failed["result"]["isError"], true);
        assert_eq!(answer_text(&failed), NO_VIEW_ERROR);
        let asks_before = driver.asks();

        let targets = call(&driver, "list_targets");
        assert_eq!(targets["result"]["isError"], false);
        assert!(answer_text(&targets).contains("no browser view is open"));
        assert_eq!(
            driver.asks(),
            asks_before,
            "the registry answers; list_targets never asks for a pane"
        );
    }

    // The registry check is the primary signal; the platform's own words are the
    // narrow second one, and a page-level failure stays an answer.
    #[test]
    fn only_a_missing_webview_is_a_view_gone() {
        assert!(missing_webview(&EmbedError::UnknownView("embed-1".into())));
        // `tauri::Error::WebviewNotFound`'s `Display`, inside the failure that
        // wraps it.
        assert!(missing_webview(&EmbedError::Navigate("webview not found".into())));
        assert!(missing_webview(&EmbedError::Eval("webview not found".into())));

        for answer in [
            EmbedError::Eval("ReferenceError: foo is not defined".into()),
            EmbedError::Eval("embed-1 did not answer within 5s".into()),
            EmbedError::Navigate("could not resolve the url".into()),
            EmbedError::InvalidUrl("file:///etc/passwd".into()),
            EmbedError::Create("no window labelled main".into()),
        ] {
            assert!(!missing_webview(&answer), "{answer} is an answer, not a retry");
        }
    }

    #[test]
    fn a_token_is_long_enough_to_not_be_guessable_and_differs_per_call() {
        let first = new_token();
        let second = new_token();
        assert_eq!(first.len(), 64, "32 bytes in hex");
        assert_ne!(first, second);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // S4: what Nabla hands to `pi mcp add`. The scope is global on purpose: the
    // project scope writes `.pi/mcp.json` inside the human's repository, and this
    // entry belongs to the app, not to the project.
    //
    // S6: the token travels in the `Authorization` header, so the url a leaked
    // `mcp.json` or a `pi mcp list` shows carries no secret.
    #[test]
    fn the_registration_is_one_global_http_server_with_the_token_in_a_header() {
        let endpoint = Endpoint {
            url: "http://127.0.0.1:51234/mcp".to_string(),
            token: "0123456789abcdef0123456789abcdef".to_string(),
        };
        let draft = registration_draft(&endpoint);

        assert_eq!(draft.name, SERVER_NAME);
        assert_eq!(draft.scope, crate::mcp::McpScope::Global);
        assert_eq!(draft.transport, crate::mcp::McpTransport::Http);
        // The mode that actually declares the tools to the model.
        assert_eq!(draft.exposure.as_deref(), Some("direct"));

        let argv = crate::mcp::add_args(&draft).expect("a valid registration draft");
        assert_eq!(
            argv,
            [
                "mcp",
                "add",
                SERVER_NAME,
                "--exposure",
                "direct",
                "--url",
                "http://127.0.0.1:51234/mcp",
                "--header",
                "Authorization=Bearer 0123456789abcdef0123456789abcdef",
            ]
            .iter()
            .map(|arg| arg.to_string())
            .collect::<Vec<_>>()
        );

        // The url is the endpoint's own, and holds no secret.
        let url = &argv[argv.iter().position(|arg| arg == "--url").expect("--url") + 1];
        assert_eq!(url, &endpoint.url);
        assert!(
            !url.contains(&endpoint.token),
            "the url must carry no secret: {url}"
        );
    }

    // S6: the endpoint is useless without the token, binds a local port that the
    // app can advertise, and refuses the methods it does not implement. Driven
    // over a real socket on purpose: this is the layer pi talks to.
    #[test]
    fn the_endpoint_requires_its_token_and_answers_json_over_http() {
        let driver = Arc::new(FakeDriver::one());
        let token = new_token();
        let endpoint = start(driver, token.clone()).expect("bind a local port");
        assert!(endpoint.url.starts_with("http://127.0.0.1:"));

        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string();

        // Without the token: refused, and nothing about the tools leaks.
        let unauthorized = post(&endpoint.url, &body, None);
        assert!(unauthorized.starts_with("HTTP/1.1 401"), "{unauthorized}");

        // With the token in the header: the catalog comes back as JSON.
        let authorized = post(&endpoint.url, &body, Some(&token));
        assert!(authorized.starts_with("HTTP/1.1 200"), "{authorized}");
        assert!(authorized.contains("\"navigate\""), "{authorized}");

        // With the token in the query instead: same answer, for a URL-only
        // registration.
        let via_query = post(&format!("{}?token={token}", endpoint.url), &body, None);
        assert!(via_query.starts_with("HTTP/1.1 200"), "{via_query}");

        // GET has no stream to offer, and saying so beats hanging.
        let get = raw(&endpoint.url, "GET");
        assert!(get.starts_with("HTTP/1.1 405"), "{get}");
    }

    /// Sends one POST over a real socket and returns the raw response.
    fn post(url: &str, body: &str, token: Option<&str>) -> String {
        let request = format!(
            "POST {} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\n{}\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            path_of(url),
            token
                .map(|token| format!("Authorization: Bearer {token}\r\n"))
                .unwrap_or_default(),
            body.len()
        );
        send(url, &request)
    }

    fn raw(url: &str, method: &str) -> String {
        let request = format!(
            "{method} {} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            path_of(url)
        );
        send(url, &request)
    }

    fn path_of(url: &str) -> &str {
        let after_host = url
            .split_once("127.0.0.1:")
            .map(|(_, rest)| rest)
            .unwrap_or("/");
        match after_host.find('/') {
            Some(index) => &after_host[index..],
            None => "/",
        }
    }

    fn send(url: &str, request: &str) -> String {
        let port: u16 = url
            .splitn(3, ':')
            .nth(2)
            .and_then(|rest| rest.split('/').next())
            .and_then(|port| port.parse().ok())
            .expect("port of the endpoint url");
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream
            .write_all(request.as_bytes())
            .expect("write the request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read the answer");
        response
    }

    // ── the registration gate (S4 of `webview-control.md`, defect L8) ───────

    // A completed gate is a value a session start reads, not a wait: the
    // registration already reported, so nothing may pause here.
    #[test]
    fn the_gate_answers_a_completed_outcome_without_waiting() {
        let gate = RegistrationGate::new();
        gate.complete(RegistrationOutcome::Registered);

        let started = Instant::now();
        let outcome = gate.wait(Duration::from_secs(30));

        assert_eq!(outcome, RegistrationOutcome::Registered);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a completed gate never waits, took {:?}",
            started.elapsed()
        );
    }

    // The defect itself (L8): a session created while the registration is still
    // running must wait for it and get its real answer, not the timeout.
    #[test]
    fn the_gate_opens_when_the_registration_completes_while_a_session_waits() {
        let gate = Arc::new(RegistrationGate::new());
        let completing = Arc::clone(&gate);
        let registration = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            completing.complete(RegistrationOutcome::Registered);
        });

        let started = Instant::now();
        let outcome = gate.wait(Duration::from_secs(10));
        let waited = started.elapsed();
        registration.join().expect("the completing thread panicked");

        assert_eq!(
            outcome,
            RegistrationOutcome::Registered,
            "the gate hands over what the registration reported"
        );
        assert!(
            waited >= Duration::from_millis(50),
            "it must have waited for the completion, returned after {waited:?}"
        );
    }

    // A registration that never reports must not hang a session: the wait is
    // bounded, and says what the silence costs instead of pretending success.
    #[test]
    fn the_gate_answers_a_timeout_when_the_registration_never_reports() {
        let gate = RegistrationGate::new();

        let started = Instant::now();
        let outcome = gate.wait(Duration::from_millis(80));
        let waited = started.elapsed();

        assert_eq!(
            outcome,
            RegistrationOutcome::Failed("the registration did not finish in time".to_string())
        );
        assert!(
            waited >= Duration::from_millis(80),
            "the wait is the timeout it was given, returned after {waited:?}"
        );
        assert!(
            waited < Duration::from_secs(5),
            "and no longer than that, took {waited:?}"
        );
    }

    // The first report is the one sessions may already have acted on, so a
    // second completion cannot rewrite it.
    #[test]
    fn completing_the_gate_twice_keeps_the_first_outcome() {
        let gate = RegistrationGate::new();
        gate.complete(RegistrationOutcome::Skipped);
        gate.complete(RegistrationOutcome::Failed("a late failure".to_string()));

        assert_eq!(gate.wait(Duration::from_secs(30)), RegistrationOutcome::Skipped);
    }

    // The failure is announced once and names its cost: every session start asks
    // the same gate, and one line is the honest amount of noise.
    #[test]
    fn a_failed_registration_is_announced_once_with_its_consequence() {
        let gate = RegistrationGate::new();
        let failed = RegistrationOutcome::Failed("pi is not installed".to_string());

        let notice = gate.announce(&failed).expect("the first failure is logged");
        assert!(notice.contains("pi is not installed"), "got {notice:?}");
        assert!(
            notice.contains("will not have the browser tools"),
            "the line names the consequence, got {notice:?}"
        );
        assert_eq!(gate.announce(&failed), None, "the failure is logged once");

        // A success has nothing to say, before or after a failure.
        let quiet = RegistrationGate::new();
        assert_eq!(quiet.announce(&RegistrationOutcome::Registered), None);
        assert_eq!(quiet.announce(&RegistrationOutcome::Skipped), None);
    }
}

/// The HTTP side needs `axum`/`tokio`; the pieces above are pure. Kept in one
/// file because the binding is thin and splitting it would hide that.
pub fn start(
    driver: Arc<dyn WebviewDriver>,
    token: String,
) -> Result<Endpoint, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| error.to_string())?;
    let port = listener.local_addr().map_err(|error| error.to_string())?.port();
    let endpoint = Endpoint {
        url: format!("http://127.0.0.1:{port}/mcp"),
        token,
    };
    serve(listener, driver, endpoint.token.clone())?;
    Ok(endpoint)
}

/// The real driver: the app's own `embed` registry on the main window.
///
/// The adapter is this thin on purpose — the tools call exactly what the UI's
/// commands call, so the agent can never reach a webview the human cannot.
pub struct EmbedDriver {
    app: tauri::AppHandle,
    embeds: crate::embed::EmbedSessions,
}

impl EmbedDriver {
    pub fn new(app: tauri::AppHandle, embeds: crate::embed::EmbedSessions) -> EmbedDriver {
        EmbedDriver { app, embeds }
    }

    /// What one embed failure means to the dispatcher, and what is corrected
    /// before answering it.
    ///
    /// Two signals, in this order:
    /// 1. the registry, which this app owns: an id it no longer holds is gone by
    ///    definition;
    /// 2. [`missing_webview`], for the orphan the registry cannot see — an entry
    ///    that outlived its webview.
    ///
    /// A `ViewGone` evicts the entry before answering, so the same dead id cannot
    /// be resolved, listed or driven twice. Nothing else is evicted: a script error
    /// or a refused url leaves a perfectly good view in place.
    fn failure(&self, view_id: &str, error: EmbedError) -> DriverFailure {
        if self.embeds.get(view_id).is_some() && !missing_webview(&error) {
            return DriverFailure::Other(error.to_string());
        }
        // The registry is the thing being corrected, not the webview: the webview
        // is already gone (that is why we are here), while the entry would keep
        // answering `views()` and keep being resolved by the next tool call.
        self.embeds.evict(view_id);
        DriverFailure::ViewGone
    }

    /// Whether a call may run at all: the registry is checked first, before the
    /// platform is asked anything, because it is the primary signal.
    fn driveable(&self, view_id: &str) -> Result<(), DriverFailure> {
        match self.embeds.get(view_id) {
            Some(_) => Ok(()),
            None => Err(DriverFailure::ViewGone),
        }
    }
}

/// Whether the platform said the **webview itself** is missing.
///
/// The registry check in [`EmbedDriver::failure`] is the primary signal and is
/// enough for the common case; this exists because one case is invisible to it: an
/// entry can outlive its webview (a creation that failed halfway through, or a
/// close racing the call), and then `views()` lists `embed-1` while Tauri has no
/// such webview.
///
/// `EmbedError::UnknownView` is this crate's own words for exactly that — the
/// registry had the id, the platform did not. The string is
/// `tauri::Error::WebviewNotFound`'s `Display`, which is what an operation on a
/// vanished webview answers. Both checks are deliberately narrow: a script error
/// or a refused url that happens to mention a webview must stay an answer, never
/// become a retry.
fn missing_webview(error: &EmbedError) -> bool {
    match error {
        EmbedError::UnknownView(_) => true,
        EmbedError::Navigate(message) | EmbedError::Eval(message) => {
            message.contains("webview not found")
        }
        _ => false,
    }
}

impl WebviewDriver for EmbedDriver {
    fn views(&self) -> Vec<LiveView> {
        self.embeds
            .views()
            .into_iter()
            .map(|view| LiveView {
                id: view.id,
                url: view.url,
            })
            .collect()
    }

    fn ensure_view(&self, timeout: Duration) -> bool {
        // Ask, then wait, in that order. The app UI owns the geometry (S3): this
        // only says "a tool needs a view", and the pane that opens creates it with
        // the rect it measured, registering it before the webview exists — which
        // is what wakes the wait. A failed emit is not special-cased: the wait
        // then answers the same way as a pane nobody opened.
        let _ = self.app.emit(crate::embed::NEED_VIEW_EVENT, ());
        self.embeds.wait_for_first_view(timeout)
    }

    fn navigate(&self, view_id: &str, url: &str) -> Result<(), DriverFailure> {
        self.driveable(view_id)?;
        self.embeds
            .navigate(&self.app, view_id, url)
            .map_err(|error| self.failure(view_id, error))
    }

    fn eval(&self, view_id: &str, script: &str) -> Result<String, DriverFailure> {
        self.driveable(view_id)?;
        self.embeds
            .eval(&self.app, view_id, script)
            .map_err(|error| self.failure(view_id, error))
    }
}

/// What one request needs to be answered: the driver, this run's token, and the
/// session id MCP clients expect to see echoed after the handshake.
struct ServerState {
    driver: Arc<dyn WebviewDriver>,
    token: String,
    session_id: String,
}

/// Binds the endpoint and serves it on its own thread.
///
/// A dedicated runtime instead of Tauri's: the server must not borrow the runtime
/// that spawns pi, so a slow request can never hold up the IPC thread, and the
/// endpoint's life is the process's.
fn serve(
    listener: TcpListener,
    driver: Arc<dyn WebviewDriver>,
    token: String,
) -> Result<(), String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let session_id = format!("nabla-{}", &token[..16.min(token.len())]);
    let state = Arc::new(ServerState {
        driver,
        token,
        session_id,
    });

    std::thread::Builder::new()
        .name("nabla-mcp".to_string())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move {
                let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                    return;
                };
                // Only `/mcp`: anything else on this port is not a protocol we
                // speak, and the router answers 404 without a handler.
                let router = Router::new()
                    .route("/mcp", post(handle_post))
                    .with_state(state);
                let _ = axum::serve(listener, router).await;
            });
        })
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// One JSON-RPC message in, one answer out. Notifications answer `202` with no
/// body, which is what the spec asks for and what a client waits on.
async fn handle_post(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    body: String,
) -> Response {
    if !authorized(&headers, &query, &state.token) {
        return json_response(
            StatusCode::UNAUTHORIZED,
            &json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": { "code": -32001, "message": "missing or wrong token" },
            }),
            &state,
        );
    }

    let Ok(parsed) = serde_json::from_str::<Value>(&body) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": { "code": -32700, "message": "body is not JSON" },
            }),
            &state,
        );
    };

    match &parsed {
        // A batch answers with the batch of its non-notification messages, and
        // with nothing at all when every one of them was a notification.
        Value::Array(messages) => {
            let answers: Vec<Value> = messages
                .iter()
                .filter_map(|message| dispatch(state.driver.as_ref(), message))
                .collect();
            if answers.is_empty() {
                return accepted(&state);
            }
            json_response(StatusCode::OK, &Value::Array(answers), &state)
        }
        message => match dispatch(state.driver.as_ref(), message) {
            Some(answer) => json_response(StatusCode::OK, &answer, &state),
            None => accepted(&state),
        },
    }
}

#[derive(serde::Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

/// The token, from the header a client would normally send or from the query a
/// url-only registration can carry. Compared without an early exit so the answer
/// time says nothing about how much of the token was right.
fn authorized(headers: &HeaderMap, query: &TokenQuery, token: &str) -> bool {
    if let Some(given) = query.token.as_deref()
        && same(given.as_bytes(), token.as_bytes())
    {
        return true;
    }
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|given| same(given.trim().as_bytes(), token.as_bytes()))
}

fn same(given: &[u8], expected: &[u8]) -> bool {
    if given.len() != expected.len() {
        return false;
    }
    given
        .iter()
        .zip(expected)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn json_response(status: StatusCode, body: &Value, state: &ServerState) -> Response {
    let mut response = (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response();
    if let Ok(value) = axum::http::HeaderValue::from_str(&state.session_id) {
        response
            .headers_mut()
            .insert(header::HeaderName::from_static("mcp-session-id"), value);
    }
    response
}

fn accepted(state: &ServerState) -> Response {
    let mut response = StatusCode::ACCEPTED.into_response();
    if let Ok(value) = axum::http::HeaderValue::from_str(&state.session_id) {
        response
            .headers_mut()
            .insert(header::HeaderName::from_static("mcp-session-id"), value);
    }
    response
}
