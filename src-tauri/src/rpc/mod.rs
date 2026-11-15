// RPC module: the wire contract between Nabla and a headless pi child process.
// Rules (spec S2, R1):
// - pi runs in `--mode rpc` and speaks NDJSON over stdin/stdout: one JSON object
//   per line, LF-delimited (`pi-rpc-manager.ts`, RPC_MODE/JSONL_NEWLINE).
// - Requests carry an incremental `id`; responses echo it (correlation lives in
//   process/mod.rs, R2). Everything else is a streamed event.
// - Types mirror the shapes pi actually sends, field name by field name, so the
//   TS contracts (R4) are a verbatim mirror of this file and never the reverse.
// - Only fields Nabla uses are kept. Unknown events survive as `Other` so a
//   newer pi cannot make the runtime drop a stream silently.
//
// Wire references (read-only clones):
// - /tmp/nabla-pi-desktops.vYixxw/FaqFirebase/src/main/pi-rpc-manager.ts
//   (frame decoding, request id generation, response correlation)
// - /tmp/nabla-pi-desktops.vYixxw/FaqFirebase/src/shared/ipc-contracts.ts
//   (event shapes, process status)

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Correlated id of the startup readiness probe. Fixed rather than incremental
/// so it can never collide with a request id and a crash-restart attempt cannot
/// confuse the previous attempt's answer with its own.
/// Mirror: `pi-rpc-manager.ts`, `STARTUP_PROBE_ID`.
pub const STARTUP_PROBE_ID: &str = "__startup_probe__";

/// Cheapest liveness command: a handful of in-memory session field reads, no
/// I/O and no model call. Any correlated answer — success or an "unknown
/// command" error that echoes our id — proves the request→response loop is live.
/// Mirror: `pi-rpc-manager.ts`, `STARTUP_PROBE_COMMAND`.
pub const STARTUP_PROBE_COMMAND: &str = "get_state";

/// Lifecycle of one pi child process, owned and supervised by the Nabla host
/// (spec S1, S4): `Spawning -> Ready -> Running -> Stopped | Crashed`.
/// `Ready` is stamped when the startup probe answers, `Running` when the first
/// request is in flight, `Crashed` on any exit the host did not ask for.
/// Mirror: `ipc-contracts.ts:246` `PiProcessStatus` ('stopped' | 'starting' |
/// 'running' | 'error'), renamed to the lifecycle spec S4 mandates;
/// 'starting' is split into `Spawning`/`Ready` and 'error'-on-exit is `Crashed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Spawning,
    Ready,
    Running,
    Stopped,
    Crashed,
}

/// One progressive slice of a message while pi streams it.
/// Mirror: `ipc-contracts.ts:476-495` `PiMessageUpdateEvent.assistantMessageEvent`
/// (`type`, `contentIndex`, `delta`). Nabla keeps the discriminator plus the two
/// fields a chat view needs to grow a line of text; `partial`, `offset`,
/// `content`, `thinking`, `toolCall` and `reason` are deliberately dropped until
/// a view uses them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessageEvent {
    /// Wire discriminator: "text_delta" | "thinking_delta" | "start" | "done" |
    /// "error" (pi's own spelling, kept verbatim).
    #[serde(rename = "type")]
    pub kind: String,
    /// Text appended by this frame. Absent on non-delta slices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    /// Index of the content block this slice belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_index: Option<u64>,
}

/// A streamed pi event, in the exact shape pi sends it (`type` discriminator,
/// camelCase payload). Synthetic Nabla events share the same envelope so the
/// webview can pump one queue (R4).
/// Mirror: `ipc-contracts.ts:461-614`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// A turn of work started. Mirror: `ipc-contracts.ts:461`.
    AgentStart,
    /// A turn of work finished, with the messages it produced.
    /// Mirror: `ipc-contracts.ts:465-468`.
    AgentEnd {
        #[serde(default)]
        messages: Vec<Value>,
    },
    /// Progressive assistant text/thinking. Mirror: `ipc-contracts.ts:476-495`.
    MessageUpdate {
        /// Not sent by pi: stamped by the session process so a forwarded event
        /// can be routed to its session without the Tauri envelope (Stark's
        /// `AgentEventPayload.session_id`, `process_manager.rs`). Absent while
        /// decoding a raw pi frame, present on everything Nabla forwards.
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "sessionId")]
        session_id: Option<String>,
        /// The assistant message being updated.
        #[serde(default)]
        message: Value,
        #[serde(rename = "assistantMessageEvent")]
        assistant_message_event: AssistantMessageEvent,
    },
    /// A tool call started. Mirror: `ipc-contracts.ts:497-502`.
    ToolExecutionStart {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(default)]
        args: Value,
    },
    /// A tool call reported progress. Mirror: `ipc-contracts.ts:504-513`.
    ToolExecutionUpdate {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(default)]
        args: Value,
        #[serde(rename = "partialResult")]
        partial_result: Value,
    },
    /// A tool call finished. Mirror: `ipc-contracts.ts:515-525`.
    ToolExecutionEnd {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(default)]
        result: Value,
        #[serde(rename = "isError")]
        is_error: bool,
    },
    /// Mirror: `ipc-contracts.ts:527`.
    TurnStart,
    /// Mirror: `ipc-contracts.ts:531-535`.
    TurnEnd {
        #[serde(default)]
        message: Value,
        #[serde(default, rename = "toolResults")]
        tool_results: Vec<Value>,
    },
    /// Synthetic Nabla event: a lifecycle transition, carrying the stderr tail
    /// only when the process died (mirrors `PiStatus.error`, see
    /// `pi-rpc-manager.ts` `getStatus`).
    ProcessStatus {
        status: RuntimeStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Synthetic Nabla event: the child exited (spec S4, "process_exit sintético
    /// con cola de stderr"). Mirror: Stark `process_manager.rs` emits
    /// `{"type":"process_exit","status":code,"error":..., "hint":...}`; Nabla
    /// keeps the exit code and the bounded stderr tail instead of a hint string.
    ProcessExit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
        #[serde(default, rename = "stderrTail")]
        stderr_tail: Vec<String>,
    },
    /// Any pi event Nabla does not model yet, kept by its wire `type` so a newer
    /// pi degrades to "unhandled" instead of "dropped without a trace".
    Other { kind: String },
}

/// A correlated answer to one request. pi marks responses with
/// `type: "response"` and echoes the request id.
/// Mirror: `ipc-contracts.ts:578-585` `PiResponseEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcResponse {
    /// Echo of the request id; the correlation key (R2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Command the response belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub success: bool,
    /// Engine's refusal reason when `success` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A request encoded for the wire: the allocated id (needed for correlation)
/// and the exact line to write to the child's stdin, newline included.
#[derive(Debug, Clone, PartialEq)]
pub struct EncodedRequest {
    pub id: String,
    pub line: String,
}

/// One decoded physical line.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// A response to one of our requests (see `RpcResponse`).
    Response(RpcResponse),
    /// A streamed event (see `AgentEvent`).
    Event(AgentEvent),
    /// A line that was not a JSON object. Reported, never fatal: a garbled line
    /// must not kill a live session or panic the reader thread.
    InvalidLine { line: String, reason: String },
}

/// NDJSON codec: buffers bytes until a whole line is available, splits
/// responses from events, and allocates the incremental request ids.
#[derive(Debug)]
pub struct FrameCodec {
    /// Bytes of a line that has not been terminated yet (a frame split across
    /// two reads, e.g. when the pipe flushes mid-object).
    buffer: Vec<u8>,
    /// Next request id; ids are decimal and start at 1.
    next_id: u64,
}

impl Default for FrameCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameCodec {
    pub fn new() -> FrameCodec {
        FrameCodec {
            buffer: Vec::new(),
            next_id: 1,
        }
    }

    /// Encodes `command` plus `fields` as one NDJSON line and allocates its id.
    /// The id is always ours: pi never invents one for a request.
    pub fn encode_request(&mut self, command: &str, mut fields: Map<String, Value>) -> EncodedRequest {
        let id = self.next_id.to_string();
        self.next_id += 1;
        // The allocated id wins over any caller-supplied one: a clobbered id
        // would resolve somebody else's pending response.
        fields.insert("id".to_string(), Value::String(id.clone()));
        fields.insert("type".to_string(), Value::String(command.to_string()));
        // An object always serializes; the fallback is unreachable bookkeeping.
        let mut line = serde_json::to_string(&Value::Object(fields)).unwrap_or_default();
        line.push('\n');
        EncodedRequest { id, line }
    }

    /// Feeds a chunk of the child's stdout and returns every frame it completed.
    /// Bytes that do not yet end a line stay buffered.
    pub fn push_bytes(&mut self, chunk: &[u8]) -> Vec<Frame> {
        self.buffer.extend_from_slice(chunk);
        let mut frames = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = self.buffer.drain(..=newline).collect();
            if let Some(frame) = self.decode_line(&raw) {
                frames.push(frame);
            }
        }
        frames
    }

    /// Flushes a trailing line that arrived without its final newline (stdout
    /// closed mid-line). Mirror: `pi-rpc-manager.ts` stdout 'end' handler.
    pub fn finish(&mut self) -> Vec<Frame> {
        let raw = std::mem::take(&mut self.buffer);
        self.decode_line(&raw).into_iter().collect()
    }

    /// Decodes one physical line. `None` means "nothing to report" (blank line).
    fn decode_line(&self, raw: &[u8]) -> Option<Frame> {
        // Lossy on purpose: a broken byte must degrade that line, not the stream.
        let text = String::from_utf8_lossy(raw);
        let line = text.trim();
        if line.is_empty() {
            return None;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(err) => {
                return Some(Frame::InvalidLine {
                    line: line.to_string(),
                    reason: err.to_string(),
                });
            }
        };
        if !value.is_object() {
            return Some(Frame::InvalidLine {
                line: line.to_string(),
                reason: "line is not a JSON object".to_string(),
            });
        }
        // A response is an object tagged `type: "response"` that echoes our id.
        // An event may carry an `id` of its own (extension_ui_request), so `id`
        // alone never decides the split. Mirror: `pi-rpc-manager.ts` handleLine.
        let Some(kind) = value.get("type").and_then(Value::as_str).map(str::to_string) else {
            return Some(Frame::InvalidLine {
                line: line.to_string(),
                reason: "missing type field".to_string(),
            });
        };
        if kind == "response" {
            return match serde_json::from_value::<RpcResponse>(value) {
                Ok(response) => Some(Frame::Response(response)),
                Err(err) => Some(Frame::InvalidLine {
                    line: line.to_string(),
                    reason: err.to_string(),
                }),
            };
        }
        match serde_json::from_value::<AgentEvent>(value) {
            Ok(event) => Some(Frame::Event(event)),
            // A type we do not model yet: keep the name, drop the payload.
            Err(_) => Some(Frame::Event(AgentEvent::Other { kind })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields(pairs: &[(&str, Value)]) -> Map<String, Value> {
        let mut map = Map::new();
        for (k, v) in pairs {
            map.insert((*k).to_string(), v.clone());
        }
        map
    }

    #[test]
    fn encode_request_is_one_lf_terminated_json_line_with_incremental_id() {
        let mut codec = FrameCodec::new();
        let first = codec.encode_request(
            "prompt",
            fields(&[("message", json!("hello"))]),
        );
        assert_eq!(first.id, "1", "ids start at 1");
        assert!(first.line.ends_with('\n'), "one line, LF terminated");
        assert_eq!(
            first.line.matches('\n').count(),
            1,
            "exactly one newline: {}",
            first.line
        );
        let parsed: Value = serde_json::from_str(first.line.trim_end()).expect("valid json");
        assert_eq!(parsed["type"], "prompt");
        assert_eq!(parsed["id"], "1");
        assert_eq!(parsed["message"], "hello");

        let second = codec.encode_request("abort", Map::new());
        assert_eq!(second.id, "2", "ids increment");
        let parsed: Value = serde_json::from_str(second.line.trim_end()).expect("valid json");
        assert_eq!(parsed["type"], "abort");
        assert_eq!(parsed["id"], "2");
        assert_eq!(
            parsed.as_object().map(|o| o.len()),
            Some(2),
            "no empty payload fields leak into the frame: {}",
            second.line
        );
    }

    #[test]
    fn encode_request_replaces_a_caller_supplied_id() {
        let mut codec = FrameCodec::new();
        // A stray id in the payload must never win over the allocated one, or
        // response correlation would answer the wrong caller.
        let encoded = codec.encode_request("get_state", fields(&[("id", json!("bogus"))]));
        assert_eq!(encoded.id, "1");
        let parsed: Value = serde_json::from_str(encoded.line.trim_end()).expect("valid json");
        assert_eq!(parsed["id"], "1");
    }

    #[test]
    fn decode_message_update_keeps_delta_and_content_index() {
        let mut codec = FrameCodec::new();
        let line = concat!(
            r#"{"type":"message_update","message":{"role":"assistant"},"#,
            r#""assistantMessageEvent":{"type":"text_delta","delta":"Hel","contentIndex":0}}"#,
            "\n"
        );
        let frames = codec.push_bytes(line.as_bytes());
        assert_eq!(frames.len(), 1);
        let Frame::Event(event) = &frames[0] else {
            panic!("expected event, got {:?}", frames[0]);
        };
        let expected = AgentEvent::MessageUpdate {
            session_id: None,
            message: json!({"role": "assistant"}),
            assistant_message_event: AssistantMessageEvent {
                kind: "text_delta".into(),
                delta: Some("Hel".into()),
                content_index: Some(0),
            },
        };
        assert_eq!(event, &expected, "typed fields must mirror pi's frame");
        // The typed event must serialize back to the same wire shape R4 mirrors.
        assert_eq!(serde_json::to_value(event).unwrap(), json!({
            "type": "message_update",
            "message": {"role": "assistant"},
            "assistantMessageEvent": {"type": "text_delta", "delta": "Hel", "contentIndex": 0},
        }));
    }

    #[test]
    fn decode_message_update_without_delta_still_decodes() {
        let mut codec = FrameCodec::new();
        let line = concat!(
            r#"{"type":"message_update","message":{"role":"assistant"},"#,
            r#""assistantMessageEvent":{"type":"start"}}"#,
            "\n"
        );
        let frames = codec.push_bytes(line.as_bytes());
        let Frame::Event(AgentEvent::MessageUpdate {
            assistant_message_event,
            ..
        }) = &frames[0]
        else {
            panic!("expected message_update, got {:?}", frames[0]);
        };
        assert_eq!(assistant_message_event.kind, "start");
        assert_eq!(assistant_message_event.delta, None);
        assert_eq!(assistant_message_event.content_index, None);
    }

    #[test]
    fn decode_tool_execution_events_typed_fields() {
        let mut codec = FrameCodec::new();
        let input = concat!(
            r#"{"type":"tool_execution_start","toolCallId":"call-1","toolName":"read","args":{"path":"a.txt"}}"#, "\n",
            r#"{"type":"tool_execution_update","toolCallId":"call-1","toolName":"read","args":{"path":"a.txt"},"partialResult":{"content":[{"type":"text","text":"a"}]}}"#, "\n",
            r#"{"type":"tool_execution_end","toolCallId":"call-1","toolName":"read","result":{"content":[{"type":"text","text":"a"}]},"isError":false}"#, "\n",
        );
        let frames = codec.push_bytes(input.as_bytes());
        assert_eq!(frames.len(), 3);
        let Frame::Event(start) = &frames[0] else { panic!("start") };
        assert_eq!(serde_json::to_value(start).unwrap(), json!({
            "type": "tool_execution_start",
            "toolCallId": "call-1",
            "toolName": "read",
            "args": {"path": "a.txt"},
        }));
        let Frame::Event(update) = &frames[1] else { panic!("update") };
        assert_eq!(serde_json::to_value(update).unwrap(), json!({
            "type": "tool_execution_update",
            "toolCallId": "call-1",
            "toolName": "read",
            "args": {"path": "a.txt"},
            "partialResult": {"content": [{"type": "text", "text": "a"}]},
        }));
        let Frame::Event(end) = &frames[2] else { panic!("end") };
        assert_eq!(serde_json::to_value(end).unwrap(), json!({
            "type": "tool_execution_end",
            "toolCallId": "call-1",
            "toolName": "read",
            "result": {"content": [{"type": "text", "text": "a"}]},
            "isError": false,
        }));
    }

    #[test]
    fn decode_line_split_across_chunks() {
        let mut codec = FrameCodec::new();
        let line = r#"{"type":"agent_start"}
"#;
        // Split inside the JSON object: nothing may be decoded from the prefix.
        assert!(codec.push_bytes(&line.as_bytes()[0..10]).is_empty());
        let frames = codec.push_bytes(&line.as_bytes()[10..]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], Frame::Event(AgentEvent::AgentStart));
    }

    #[test]
    fn decode_several_events_from_one_chunk() {
        let mut codec = FrameCodec::new();
        let chunk = "{\"type\":\"agent_start\"}\n{\"type\":\"turn_start\"}\n";
        let frames = codec.push_bytes(chunk.as_bytes());
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], Frame::Event(AgentEvent::AgentStart));
        assert_eq!(frames[1], Frame::Event(AgentEvent::TurnStart));
    }

    #[test]
    fn invalid_json_line_reports_error_and_codec_keeps_working() {
        let mut codec = FrameCodec::new();
        let chunk = "not json\n{\"type\":\"agent_start\"}\n";
        let frames = codec.push_bytes(chunk.as_bytes());
        assert_eq!(frames.len(), 2, "a bad line must not stop the stream");
        match &frames[0] {
            Frame::InvalidLine { line, reason } => {
                assert_eq!(line, "not json");
                assert!(!reason.is_empty());
            }
            other => panic!("expected InvalidLine, got {other:?}"),
        }
        assert_eq!(frames[1], Frame::Event(AgentEvent::AgentStart));
    }

    #[test]
    fn invalid_json_object_without_type_is_reported_not_typed() {
        let mut codec = FrameCodec::new();
        let frames = codec.push_bytes(b"{\"no\":1}\n");
        assert_eq!(frames.len(), 1);
        assert!(matches!(&frames[0], Frame::InvalidLine { .. }));
    }

    #[test]
    fn response_is_separated_from_events_and_ids_are_kept() {
        let mut codec = FrameCodec::new();
        let chunk = concat!(
            r#"{"type":"response","command":"prompt","id":"7","success":true,"data":null}"#, "\n",
            r#"{"type":"extension_ui_request","id":"ui-1","method":"confirm","message":"sure?"}"#, "\n"
        );
        let frames = codec.push_bytes(chunk.as_bytes());
        assert_eq!(frames.len(), 2);
        let expected = Frame::Response(RpcResponse {
            id: Some("7".into()),
            command: Some("prompt".into()),
            success: true,
            error: None,
            data: None,
        });
        assert_eq!(frames[0], expected);
        // An event that happens to carry an `id` must not be mistaken for a
        // response (extension_ui_request has one, ipc-contracts.ts:619).
        assert!(matches!(
            &frames[1],
            Frame::Event(AgentEvent::Other { kind }) if kind == "extension_ui_request"
        ));
    }

    #[test]
    fn response_without_id_or_error_still_decodes() {
        let mut codec = FrameCodec::new();
        let frames = codec.push_bytes(b"{\"type\":\"response\",\"success\":false,\"error\":\"unknown command\"}\n");
        let Frame::Response(response) = &frames[0] else {
            panic!("expected response, got {:?}", frames[0]);
        };
        assert_eq!(response.id, None);
        assert!(!response.success);
        assert_eq!(response.error.as_deref(), Some("unknown command"));
    }

    #[test]
    fn crlf_and_blank_lines_are_tolerated() {
        let mut codec = FrameCodec::new();
        let frames = codec.push_bytes(b"\r\n{\"type\":\"agent_start\"}\r\n\n");
        assert_eq!(frames.len(), 1, "blank lines carry nothing: {frames:?}");
        assert_eq!(frames[0], Frame::Event(AgentEvent::AgentStart));
    }

    #[test]
    fn finish_flushes_a_trailing_line_without_newline() {
        let mut codec = FrameCodec::new();
        assert!(codec.push_bytes(b"{\"type\":\"agent_start\"}").is_empty());
        let frames = codec.finish();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], Frame::Event(AgentEvent::AgentStart));
        // Nothing is left buffered after a flush.
        assert!(codec.finish().is_empty());
        assert!(codec.push_bytes(b"{\"type\":\"turn_start\"}\n").len() == 1);
    }

    #[test]
    fn decode_agent_end_and_turn_end_arrays() {
        let mut codec = FrameCodec::new();
        let chunk = concat!(
            r#"{"type":"agent_end","messages":[{"role":"assistant"}]}"#, "\n",
            r#"{"type":"turn_end","message":{"role":"assistant"},"toolResults":[]}"#, "\n"
        );
        let frames = codec.push_bytes(chunk.as_bytes());
        let Frame::Event(AgentEvent::AgentEnd { messages }) = &frames[0] else {
            panic!("expected agent_end, got {:?}", frames[0]);
        };
        assert_eq!(messages, &vec![json!({"role": "assistant"})]);
        let Frame::Event(AgentEvent::TurnEnd {
            message,
            tool_results,
        }) = &frames[1]
        else {
            panic!("expected turn_end, got {:?}", frames[1]);
        };
        assert_eq!(message, &json!({"role": "assistant"}));
        assert!(tool_results.is_empty());
    }

    #[test]
    fn synthetic_process_events_serialize_for_the_webview() {
        // The webview mirrors these names verbatim (R4), so pin them here.
        let status = AgentEvent::ProcessStatus {
            status: RuntimeStatus::Crashed,
            error: Some("boom".into()),
        };
        assert_eq!(serde_json::to_value(&status).unwrap(), json!({
            "type": "process_status",
            "status": "crashed",
            "error": "boom",
        }));
        let exit = AgentEvent::ProcessExit {
            code: Some(2),
            stderr_tail: vec!["line one".into()],
        };
        assert_eq!(serde_json::to_value(&exit).unwrap(), json!({
            "type": "process_exit",
            "code": 2,
            "stderrTail": ["line one"],
        }));
        // No stderr tail and no code -> both fields are omitted, never null.
        assert_eq!(
            serde_json::to_value(AgentEvent::ProcessExit {
                code: None,
                stderr_tail: Vec::new()
            })
            .unwrap(),
            json!({"type": "process_exit", "stderrTail": []})
        );
    }
}
