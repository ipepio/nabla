//! Agent Client Protocol (ACP) wire translation layer.
//!
//! Translates ACP JSON-RPC 2.0 requests, responses, and `session/update`
//! notifications to and from Nabla's `AgentEvent` stream so the Vue webview
//! renders ACP sessions identically to core Nabla sessions.

use serde_json::{Value, json};

use crate::mcp_server::Endpoint;
use crate::rpc::{AgentEvent, AssistantMessageEvent, StopReason};

/// ACP protocol version currently implemented (integer `1` per ACP spec).
pub const ACP_PROTOCOL_VERSION: u64 = 1;

/// Builds the ACP `initialize` JSON-RPC request.
pub fn build_initialize_request(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": ACP_PROTOCOL_VERSION,
            "clientCapabilities": {
                "fs": {
                    "readTextFile": false,
                    "writeTextFile": false
                },
                "terminal": false
            },
            "clientInfo": {
                "name": "Nabla",
                "title": "Nabla",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    })
}

/// Builds the ACP `authenticate` JSON-RPC request.
pub fn build_authenticate_request(id: u64, method_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "authenticate",
        "params": {
            "methodId": method_id
        }
    })
}

/// Builds the ACP `session/new` JSON-RPC request, injecting Nabla's embedded
/// HTTP MCP server (`nabla-browser`) so the ACP agent inherits Browser,
/// Terminal, Drawer, Document, Web Search, Memory, Session Context, and Subagents.
pub fn build_session_new_request(id: u64, cwd: &str, mcp: Option<&Endpoint>) -> Value {
    let mut mcp_servers = Vec::new();
    if let Some(ep) = mcp {
        mcp_servers.push(json!({
            "type": "http",
            "name": crate::mcp_server::SERVER_NAME,
            "url": ep.url,
            "headers": [
                {
                    "name": "Authorization",
                    "value": format!("Bearer {}", ep.token)
                }
            ]
        }));
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/new",
        "params": {
            "cwd": cwd,
            "mcpServers": mcp_servers
        }
    })
}

/// Builds the ACP `session/set_model` JSON-RPC request to switch models live
/// inside an already-running ACP session without losing context.
pub fn build_session_set_model_request(id: u64, acp_session_id: &str, model_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/set_model",
        "params": {
            "sessionId": acp_session_id,
            "modelId": model_id
        }
    })
}

/// Builds the ACP `session/prompt` JSON-RPC request.
pub fn build_session_prompt_request(
    id: u64,
    acp_session_id: &str,
    text: &str,
    images: &[(String, String)],
    system_context: Option<&str>,
    handoff_context: Option<&str>,
) -> Value {
    let mut prompt = Vec::new();
    if let Some(ctx) = system_context
        && !ctx.trim().is_empty()
    {
        prompt.push(json!({
            "type": "text",
            "text": format!("<nabla_environment>\n{}\n</nabla_environment>", ctx.trim())
        }));
    }
    if let Some(history) = handoff_context
        && !history.trim().is_empty()
    {
        prompt.push(json!({
            "type": "text",
            "text": history.trim()
        }));
    }
    if !text.is_empty() {
        prompt.push(json!({
            "type": "text",
            "text": text
        }));
    }
    for (data, mime_type) in images {
        prompt.push(json!({
            "type": "image",
            "data": data,
            "mimeType": mime_type
        }));
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/prompt",
        "params": {
            "sessionId": acp_session_id,
            "prompt": prompt
        }
    })
}

/// Builds the ACP `session/cancel` JSON-RPC notification.
pub fn build_session_cancel_notification(acp_session_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": {
            "sessionId": acp_session_id
        }
    })
}

/// Builds a JSON-RPC response to `session/request_permission`.
pub fn build_permission_response(request_id: &Value, selected_option_id: Option<&str>) -> Value {
    let outcome = match selected_option_id {
        Some(opt_id) => json!({
            "outcome": "selected",
            "optionId": opt_id
        }),
        None => json!({
            "outcome": "cancelled"
        }),
    };
    json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {
            "outcome": outcome
        }
    })
}

/// Translates an ACP `stopReason` string into Nabla's `StopReason`.
pub fn map_stop_reason(raw: Option<&str>) -> StopReason {
    match raw.unwrap_or("end_turn") {
        "end_turn" | "stop" => StopReason::Stop,
        "max_tokens" | "max_turn_requests" => StopReason::Length,
        "cancelled" | "aborted" => StopReason::Aborted,
        "refusal" => StopReason::Error,
        other => StopReason::Unknown(other.to_string()),
    }
}

/// Translates an ACP `session/update` notification's `params` into zero or more
/// `AgentEvent`s and optionally returns text appended to the assistant message.
pub fn translate_session_update(
    session_key: &str,
    params: &Value,
) -> (Vec<AgentEvent>, Option<String>) {
    let update = match params.get("update") {
        Some(u) => u,
        None => return (Vec::new(), None),
    };
    let kind = update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .unwrap_or("");

    match kind {
        "agent_message_chunk" => {
            let text = extract_chunk_text(update);
            if text.is_empty() {
                return (Vec::new(), None);
            }
            let event = AgentEvent::MessageUpdate {
                session_id: Some(session_key.to_string()),
                message: json!({ "role": "assistant" }),
                assistant_message_event: AssistantMessageEvent {
                    kind: "text_delta".to_string(),
                    delta: Some(text.clone()),
                    content_index: Some(0),
                },
            };
            (vec![event], Some(text))
        }
        "agent_thought_chunk" => {
            let text = extract_chunk_text(update);
            if text.is_empty() {
                return (Vec::new(), None);
            }
            let event = AgentEvent::MessageUpdate {
                session_id: Some(session_key.to_string()),
                message: json!({ "role": "assistant" }),
                assistant_message_event: AssistantMessageEvent {
                    kind: "thinking_delta".to_string(),
                    delta: Some(text),
                    content_index: Some(0),
                },
            };
            (vec![event], None)
        }
        "tool_call" => {
            let tool_call_id = update
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let tool_name = update
                .get("title")
                .and_then(Value::as_str)
                .or_else(|| update.get("kind").and_then(Value::as_str))
                .unwrap_or("tool")
                .to_string();
            let args = update
                .get("rawInput")
                .cloned()
                .unwrap_or_else(|| json!({}));
            (
                vec![AgentEvent::ToolExecutionStart {
                    tool_call_id,
                    tool_name,
                    args,
                }],
                None,
            )
        }
        "tool_call_update" => {
            let tool_call_id = update
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let tool_name = update
                .get("title")
                .and_then(Value::as_str)
                .or_else(|| update.get("kind").and_then(Value::as_str))
                .unwrap_or("tool")
                .to_string();
            let status = update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("in_progress");
            let result = extract_tool_result(update);
            match status {
                "completed" | "failed" => (
                    vec![AgentEvent::ToolExecutionEnd {
                        tool_call_id,
                        tool_name,
                        result,
                        is_error: status == "failed",
                    }],
                    None,
                ),
                _ => (
                    vec![AgentEvent::ToolExecutionUpdate {
                        tool_call_id,
                        tool_name,
                        args: update
                            .get("rawInput")
                            .cloned()
                            .unwrap_or_else(|| json!({})),
                        partial_result: result,
                    }],
                    None,
                ),
            }
        }
        _ => (Vec::new(), None),
    }
}

fn extract_chunk_text(update: &Value) -> String {
    if let Some(content) = update.get("content") {
        if let Some(text) = content.get("text").and_then(Value::as_str) {
            return text.to_string();
        }
        if let Some(arr) = content.as_array() {
            let mut out = String::new();
            for item in arr {
                if let Some(t) = item.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                }
            }
            return out;
        }
    }
    String::new()
}

fn extract_tool_result(update: &Value) -> Value {
    if let Some(raw_out) = update.get("rawOutput")
        && !raw_out.is_null()
    {
        return raw_out.clone();
    }
    if let Some(content) = update.get("content").and_then(Value::as_array) {
        let mut texts = Vec::new();
        for item in content {
            if let Some(text) = item
                .get("content")
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str)
            {
                texts.push(text.to_string());
            } else if let Some(text) = item.get("text").and_then(Value::as_str) {
                texts.push(text.to_string());
            }
        }
        if !texts.is_empty() {
            return json!({
                "content": [{ "type": "text", "text": texts.join("\n") }]
            });
        }
    }
    json!({})
}

/// Parses an ACP `session/request_permission` params object.
pub fn parse_permission_request(
    params: &Value,
) -> (String, Option<String>, Vec<(String, String, String)>) {
    let tool_title = params
        .get("toolCall")
        .and_then(|tc| tc.get("title").and_then(Value::as_str))
        .unwrap_or("Permission requested");
    let raw_input = params
        .get("toolCall")
        .and_then(|tc| tc.get("rawInput"))
        .and_then(|v| serde_json::to_string_pretty(v).ok());
    let options = params
        .get("options")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|opt| {
                    let id = opt.get("optionId").and_then(Value::as_str)?.to_string();
                    let name = opt
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_string();
                    let kind = opt
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    Some((id, name, kind))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    (tool_title.to_string(), raw_input, options)
}

/// Picks the best auto-approval option id for YOLO mode (`allow_always` > `allow_once`).
pub fn pick_auto_allow_option(options: &[(String, String, String)]) -> Option<String> {
    options
        .iter()
        .find(|(_, _, kind)| kind == "allow_always")
        .or_else(|| options.iter().find(|(_, _, kind)| kind == "allow_once"))
        .or_else(|| {
            options.iter().find(|(_, name, _)| {
                let lower = name.to_ascii_lowercase();
                lower.contains("allow") || lower.contains("approve")
            })
        })
        .map(|(id, _, _)| id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_new_injects_nabla_mcp_endpoint() {
        let ep = Endpoint {
            url: "http://127.0.0.1:41234/mcp".to_string(),
            token: "secret-token".to_string(),
        };
        let req = build_session_new_request(2, "/tmp/ws", Some(&ep));
        assert_eq!(req["method"], "session/new");
        assert_eq!(req["params"]["mcpServers"][0]["name"], "nabla-browser");
    }

    #[test]
    fn builds_set_model_and_handoff_prompt() {
        let sm = build_session_set_model_request(3, "sid-1", "gemini-4-neon-medium");
        assert_eq!(sm["method"], "session/set_model");
        assert_eq!(sm["params"]["modelId"], "gemini-4-neon-medium");

        let pr = build_session_prompt_request(
            4,
            "sid-1",
            "Continue",
            &[],
            Some("sys"),
            Some("<conversation_history>prev</conversation_history>"),
        );
        assert_eq!(pr["params"]["prompt"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn translates_text_and_thinking_chunks() {
        let text_params = json!({
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": "Hola" }
            }
        });
        let (events, chunk) = translate_session_update("42", &text_params);
        assert_eq!(chunk.as_deref(), Some("Hola"));
        assert_eq!(events.len(), 1);
    }
}
