//! The MCP **server**: the bridge that lets pi drive the embedded webview.
//!
//! The webview already has its commands (`embed_*`), and `mcp.rs` already covers
//! pi's *client* side — adding, listing and logging into MCP servers. What was
//! missing is the other end: something pi can connect to in order to reach the
//! view the human is looking at (`odd/tasks/webview-control.md`, S1-S2).
//!
//! The shape is deliberately boring. A local HTTP endpoint speaks MCP's
//! *streamable HTTP* binding over JSON-RPC 2.0, exposes two tools for now, and
//! each tool calls the same driver the UI's commands call. No new protocol, no
//! second browser: the agent drives the page the human already sees.
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
use std::sync::Arc;

use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{json, Value};

/// The name pi will see in `tools/list` sources and in its own tool names
/// (`mcp__nabla-browser__navigate`).
pub const SERVER_NAME: &str = "nabla-browser";

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

    /// Navigates one existing view. The url is validated by the driver, which is
    /// where the `http`/`https`/`about:blank` rule already lives.
    fn navigate(&self, view_id: &str, url: &str) -> Result<(), String>;

    /// Runs a script in one existing view and answers what the platform
    /// serialized from it.
    fn eval(&self, view_id: &str, script: &str) -> Result<String, String>;
}

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
fn call_tool(driver: &dyn WebviewDriver, id: &Value, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if !matches!(name, "navigate" | "eval") {
        return error(id, -32602, &format!("unknown tool `{name}`"));
    }
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    let asked_view = arguments.get("viewId").and_then(Value::as_str);

    let views = driver.views();
    let view = match resolve_view(&views, asked_view) {
        Ok(view) => view,
        Err(message) => return tool_result(id, &message, true),
    };

    let outcome = match name {
        "navigate" => {
            let url = arguments.get("url").and_then(Value::as_str).unwrap_or("");
            if url.is_empty() {
                return error(id, -32602, "navigate needs a `url` argument");
            }
            driver
                .navigate(&view.id, url)
                .map(|()| format!("navigated `{}` to {url}", view.id))
        }
        _ => {
            let script = arguments.get("script").and_then(Value::as_str).unwrap_or("");
            if script.is_empty() {
                return error(id, -32602, "eval needs a `script` argument");
            }
            driver.eval(&view.id, script)
        }
    };

    match outcome {
        Ok(text) => tool_result(id, &text, false),
        Err(message) => tool_result(id, &message, true),
    }
}

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
        [] => Err(
            "no browser view is open; ask the human to open the browser pane first".to_string(),
        ),
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
        .map(|view| format!("{} ({})", view.id, view.url))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The two tools of T1. The schemas are what the model sees, so every field is
/// described: a bare `{"type":"object"}` makes the model guess.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::Mutex;

    /// A driver that records what it was asked and answers what the test set.
    struct FakeDriver {
        views: Vec<LiveView>,
        navigated: Mutex<Vec<(String, String)>>,
        evaluated: Mutex<Vec<(String, String)>>,
        eval_answer: Result<String, String>,
    }

    impl FakeDriver {
        fn with_views(views: Vec<LiveView>) -> FakeDriver {
            FakeDriver {
                views,
                navigated: Mutex::new(Vec::new()),
                evaluated: Mutex::new(Vec::new()),
                eval_answer: Ok("\"ok\"".to_string()),
            }
        }

        fn one() -> FakeDriver {
            FakeDriver::with_views(vec![LiveView {
                id: "embed-1".to_string(),
                url: "https://example.com/".to_string(),
            }])
        }
    }

    impl WebviewDriver for FakeDriver {
        fn views(&self) -> Vec<LiveView> {
            self.views.clone()
        }

        fn navigate(&self, view_id: &str, url: &str) -> Result<(), String> {
            self.navigated
                .lock()
                .unwrap()
                .push((view_id.to_string(), url.to_string()));
            Ok(())
        }

        fn eval(&self, view_id: &str, script: &str) -> Result<String, String> {
            self.evaluated
                .lock()
                .unwrap()
                .push((view_id.to_string(), script.to_string()));
            self.eval_answer.clone()
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

    // S1: the model discovers what it can do, with a schema it can fill.
    #[test]
    fn tools_list_exposes_navigate_and_eval_with_schemas() {
        let driver = FakeDriver::one();
        let answer = dispatch(&driver, &request("tools/list", json!({}))).expect("answer");
        let tools = answer["result"]["tools"].as_array().expect("tools array");

        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, vec!["navigate", "eval"]);
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(
                tool["inputSchema"]["required"].as_array().is_some_and(|r| !r.is_empty()),
                "a tool without a required argument makes the model guess"
            );
        }
        assert_eq!(tools[0]["inputSchema"]["required"], json!(["url"]));
        assert_eq!(tools[1]["inputSchema"]["required"], json!(["script"]));
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
        let mut driver = FakeDriver::one();
        driver.eval_answer = Ok("{\"title\":\"Example Domain\"}".to_string());
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
        let mut driver = FakeDriver::one();
        driver.eval_answer = Err("ReferenceError: foo is not defined".to_string());
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
    }

    #[test]
    fn a_token_is_long_enough_to_not_be_guessable_and_differs_per_call() {
        let first = new_token();
        let second = new_token();
        assert_eq!(first.len(), 64, "32 bytes in hex");
        assert_ne!(first, second);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
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

    fn navigate(&self, view_id: &str, url: &str) -> Result<(), String> {
        self.embeds
            .navigate(&self.app, view_id, url)
            .map_err(|error| error.to_string())
    }

    fn eval(&self, view_id: &str, script: &str) -> Result<String, String> {
        self.embeds
            .eval(&self.app, view_id, script)
            .map_err(|error| error.to_string())
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
