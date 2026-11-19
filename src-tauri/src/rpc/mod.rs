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

/// Why one assistant message stopped (S3 of `turn-continuity.md`).
///
/// pi's own vocabulary, verbatim (`docs/message-types.md`): `pending`,
/// `stop`, `length`, `toolUse`, `error`, `aborted`, `deferred`. The reason is
/// pi's, not Nabla's, so a word this build does not know becomes `Unknown` and
/// still crosses the bridge: a newer pi must degrade to "a reason nobody
/// announces" instead of making the whole `turn_end` frame undecodable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Pending,
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
    Deferred,
    /// A word from a pi newer than this build, kept exactly as it arrived.
    Unknown(String),
}

impl StopReason {
    /// Decodes one wire word. Total by design: nothing here can fail.
    pub fn from_wire(word: &str) -> StopReason {
        match word {
            "pending" => StopReason::Pending,
            "stop" => StopReason::Stop,
            "length" => StopReason::Length,
            "toolUse" => StopReason::ToolUse,
            "error" => StopReason::Error,
            "aborted" => StopReason::Aborted,
            "deferred" => StopReason::Deferred,
            other => StopReason::Unknown(other.to_string()),
        }
    }

    /// The word pi put on the wire; `Unknown` echoes its own.
    pub fn as_wire(&self) -> &str {
        match self {
            StopReason::Pending => "pending",
            StopReason::Stop => "stop",
            StopReason::Length => "length",
            StopReason::ToolUse => "toolUse",
            StopReason::Error => "error",
            StopReason::Aborted => "aborted",
            StopReason::Deferred => "deferred",
            StopReason::Unknown(word) => word,
        }
    }

    /// The reason an assistant message carries, or `None` when it carries none
    /// (or carries something that is not a word).
    pub fn from_message(message: &Value) -> Option<StopReason> {
        message
            .get("stopReason")
            .and_then(Value::as_str)
            .map(StopReason::from_wire)
    }
}

/// Serde writes the wire word, so a reason stays one string on the bridge.
impl Serialize for StopReason {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_wire())
    }
}

/// And reads one: decoding never rejects a word (`StopReason::from_wire`).
impl<'de> Deserialize<'de> for StopReason {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let word = String::deserialize(deserializer)?;
        Ok(StopReason::from_wire(&word))
    }
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
        /// Why the turn ended, lifted out of `message.stopReason` while the frame
        /// is decoded (S3 of `turn-continuity.md`): absent on a raw pi frame,
        /// present on everything Nabla forwards.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "stopReason"
        )]
        stop_reason: Option<StopReason>,
        /// pi's own explanation for an `error` stop, lifted the same way.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "errorMessage"
        )]
        error_message: Option<String>,
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
    /// A question an extension is asking the human (S2 of
    /// `agent-question-bridge.md`): the child emitted `extension_ui_request` and
    /// blocks until the host answers on stdin. Only the four dialog methods are
    /// typed; the fire-and-forget ones stay in `Other` (see
    /// `is_dialog_ui_method`).
    ///
    /// Wire reference: `pi/docs/rpc-extension-ui.md` —
    /// `{type, id, method, title, options?, message?, placeholder?, timeout?}`.
    /// `timeout` is deliberately dropped: Nabla's extension never sets one (S4),
    /// and a timeout is pi's own auto-resolution, not something the card renders.
    ExtensionUiRequest {
        /// The id pi minted for this question; the answer echoes it verbatim.
        id: String,
        /// Wire discriminator, kept verbatim: "select" | "confirm" | "input" |
        /// "editor". A string rather than an enum so a newer pi's method reaches
        /// the card instead of becoming an undecodable line.
        method: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// `select` only: the items the human chooses from.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<Vec<String>>,
        /// `confirm` only: the text under the title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// `input` only, optional on the wire.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
    },
    /// pi confirmed the thinking level (effort) it is now using (S2 of
    /// `composer-inputs.md`): `set_thinking_level` clamps the request to what the
    /// model accepts, so this event — never the value the UI sent — is what the
    /// effort selector renders. It also arrives when pi changes the level on its
    /// own (a model switch, a cycle).
    ///
    /// Wire reference: pi docs/json.md:124 — `{type, level}`. `level` is a string
    /// rather than an enum so a word from a newer pi still reaches the selector
    /// instead of demoting the frame to `Other` and leaving it stale.
    ThinkingLevelChanged {
        level: String,
    },
    /// Any pi event Nabla does not model yet, kept by its wire `type` so a newer
    /// pi degrades to "unhandled" instead of "dropped without a trace".
    Other { kind: String },
}

/// The line that answers one `extension_ui_request`, in the field order the
/// protocol documents (`{"type":"extension_ui_response","id":<uuid>,...}`):
/// serde_json serializes struct fields in declaration order, and only the one
/// answer field that is set is emitted.
#[derive(Serialize)]
struct UiResponseLine<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confirmed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cancelled: Option<bool>,
}

/// Builds the exact line written to the child's stdin to answer one
/// `extension_ui_request` (S2): the host's half of pi's extension UI
/// sub-protocol — LF terminated, one line, and *not* a request. pi allocates the
/// id of a question and sends no response to the answer, so `FrameCodec` must
/// never mint an id for it.
///
/// Wire reference: `pi/docs/rpc-extension-ui.md`, "Responses to Pi".
/// `cancelled` is written as given: the protocol only defines a cancellation, and
/// `src/contracts/types.ts` types it as the literal `true`.
pub fn encode_ui_response(request_id: &str, answer: &UiAnswer) -> String {
    let (value, confirmed, cancelled) = match answer {
        UiAnswer::Value { value } => (Some(value.as_str()), None, None),
        UiAnswer::Confirmed { confirmed } => (None, Some(*confirmed), None),
        UiAnswer::Cancelled { cancelled } => (None, None, Some(*cancelled)),
    };
    let response = UiResponseLine {
        kind: "extension_ui_response",
        id: request_id,
        value,
        confirmed,
        cancelled,
    };
    // An object of scalars always serializes; the fallback is unreachable
    // bookkeeping, and an empty line would be rejected by pi's parser.
    let mut line = serde_json::to_string(&response).unwrap_or_default();
    line.push('\n');
    line
}

/// The methods that wait for an answer from the human. Everything else that pi
/// sends under `extension_ui_request` (`notify`, `setStatus`, `setWidget`,
/// `setTitle`, `set_editor_text`) is fire-and-forget: typing it as a question
/// would put a card in the chat that nobody could ever answer (S4 non-goals).
fn is_dialog_ui_method(method: &str) -> bool {
    matches!(method, "select" | "confirm" | "input" | "editor")
}

/// Lifts the end-of-turn report out of `turn_end`'s `message` (S3 of
/// `turn-continuity.md`).
///
/// The message stays whole and untouched — it is pi's shape, not ours — but the
/// reason it ended becomes typed fields beside it, so the webview reads
/// `{ stopReason, errorMessage }` instead of poking into a passthrough object.
/// One place, one rule: every frame Nabla forwards has gone through here.
fn with_turn_report(mut event: AgentEvent) -> AgentEvent {
    if let AgentEvent::TurnEnd {
        message,
        stop_reason,
        error_message,
        ..
    } = &mut event
    {
        *stop_reason = StopReason::from_message(message);
        *error_message = message
            .get("errorMessage")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    event
}

/// One answer to an `extension_ui_request`, exactly as the webview sends it
/// across the bridge (S3): a chosen/typed value, a confirmation, or a cancel.
/// Mirrors `UiAnswer` in `src/contracts/types.ts`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum UiAnswer {
    /// `select` and `input`: the chosen item or the typed text.
    Value { value: String },
    /// `confirm`: the human's yes or no.
    Confirmed { confirmed: bool },
    /// Any dialog: the human dismissed it without answering.
    Cancelled { cancelled: bool },
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
            Ok(event) => {
                // `extension_ui_request` is one wire type for ten methods. Only
                // the dialogs are modeled; the fire-and-forget ones keep falling
                // through to `Other`, exactly as they did before the bridge.
                if let AgentEvent::ExtensionUiRequest { method, .. } = &event
                    && !is_dialog_ui_method(method)
                {
                    return Some(Frame::Event(AgentEvent::Other { kind }));
                }
                Some(Frame::Event(with_turn_report(event)))
            }
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
        // response (extension_ui_request has one, ipc-contracts.ts:619). Since
        // the question bridge it decodes into its own typed variant instead of
        // dying in `Other`.
        assert!(matches!(
            &frames[1],
            Frame::Event(AgentEvent::ExtensionUiRequest { id, method, .. })
                if id == "ui-1" && method == "confirm"
        ));
    }

    #[test]
    fn decode_extension_ui_request_select_keeps_the_options() {
        let mut codec = FrameCodec::new();
        let line = concat!(
            r#"{"type":"extension_ui_request","id":"ui-select","method":"select","#,
            r#""title":"Allow dangerous command?","options":["Allow","Block"]}"#, "\n"
        );
        let frames = codec.push_bytes(line.as_bytes());
        assert_eq!(frames.len(), 1);
        let Frame::Event(event) = &frames[0] else {
            panic!("expected event, got {:?}", frames[0]);
        };
        let expected = AgentEvent::ExtensionUiRequest {
            id: "ui-select".into(),
            method: "select".into(),
            title: Some("Allow dangerous command?".into()),
            options: Some(vec!["Allow".into(), "Block".into()]),
            message: None,
            placeholder: None,
        };
        assert_eq!(event, &expected, "typed fields must mirror pi's frame");
        // The webview mirrors this exact shape (R4, B3), so pin it here.
        assert_eq!(serde_json::to_value(event).unwrap(), json!({
            "type": "extension_ui_request",
            "id": "ui-select",
            "method": "select",
            "title": "Allow dangerous command?",
            "options": ["Allow", "Block"],
        }));
    }

    #[test]
    fn decode_extension_ui_request_confirm_keeps_the_message() {
        let mut codec = FrameCodec::new();
        let line = concat!(
            r#"{"type":"extension_ui_request","id":"ui-confirm","method":"confirm","#,
            r#""title":"Clear session?","message":"All messages will be lost."}"#, "\n"
        );
        let frames = codec.push_bytes(line.as_bytes());
        let Frame::Event(event) = &frames[0] else {
            panic!("expected event, got {:?}", frames[0]);
        };
        assert_eq!(serde_json::to_value(event).unwrap(), json!({
            "type": "extension_ui_request",
            "id": "ui-confirm",
            "method": "confirm",
            "title": "Clear session?",
            "message": "All messages will be lost.",
        }));
    }

    #[test]
    fn decode_extension_ui_request_input_keeps_the_placeholder_when_present() {
        let mut codec = FrameCodec::new();
        let chunk = concat!(
            r#"{"type":"extension_ui_request","id":"ui-input","method":"input","#,
            r#""title":"Enter a value","placeholder":"type something..."}"#, "\n",
            r#"{"type":"extension_ui_request","id":"ui-input-2","method":"input","#,
            r#""title":"Enter a value"}"#, "\n",
        );
        let frames = codec.push_bytes(chunk.as_bytes());
        assert_eq!(frames.len(), 2);
        let Frame::Event(with_placeholder) = &frames[0] else {
            panic!("expected event, got {:?}", frames[0]);
        };
        assert_eq!(serde_json::to_value(with_placeholder).unwrap(), json!({
            "type": "extension_ui_request",
            "id": "ui-input",
            "method": "input",
            "title": "Enter a value",
            "placeholder": "type something...",
        }));
        // Without a placeholder the field is absent on the wire and stays absent
        // in the mirror: never `null`, which a TS optional would also reject.
        let Frame::Event(without) = &frames[1] else {
            panic!("expected event, got {:?}", frames[1]);
        };
        assert_eq!(serde_json::to_value(without).unwrap(), json!({
            "type": "extension_ui_request",
            "id": "ui-input-2",
            "method": "input",
            "title": "Enter a value",
        }));
    }

    #[test]
    fn decode_extension_ui_request_ignores_the_optional_timeout() {
        // A dialog may carry a timeout (S4: our extension never sets one). It is
        // not part of what the card renders, so decoding must not depend on it.
        let mut codec = FrameCodec::new();
        let line = concat!(
            r#"{"type":"extension_ui_request","id":"ui-timed","method":"confirm","#,
            r#""title":"Timed","message":"Five seconds.","timeout":5000}"#, "\n"
        );
        let frames = codec.push_bytes(line.as_bytes());
        assert!(matches!(
            &frames[0],
            Frame::Event(AgentEvent::ExtensionUiRequest { id, method, message, .. })
                if id == "ui-timed" && method == "confirm" && message.as_deref() == Some("Five seconds.")
        ));
    }

    #[test]
    fn fire_and_forget_ui_requests_stay_untyped() {
        // `notify`, `setStatus`, `setWidget`, `setTitle` and `set_editor_text`
        // share the wire type but never wait for an answer (S4 non-goals).
        // Typing them would put a question in the chat that nobody can answer.
        let mut codec = FrameCodec::new();
        let chunk = concat!(
            r#"{"type":"extension_ui_request","id":"n-1","method":"notify","message":"hi","notifyType":"info"}"#, "\n",
            r#"{"type":"extension_ui_request","id":"s-1","method":"setStatus","statusKey":"k","statusText":"t"}"#, "\n",
            r#"{"type":"extension_ui_request","id":"w-1","method":"setWidget","widgetKey":"k","widgetLines":["a"]}"#, "\n",
            r#"{"type":"extension_ui_request","id":"t-1","method":"setTitle","title":"pi"}"#, "\n",
            r#"{"type":"extension_ui_request","id":"e-1","method":"set_editor_text","text":"x"}"#, "\n",
        );
        let frames = codec.push_bytes(chunk.as_bytes());
        assert_eq!(frames.len(), 5);
        for frame in &frames {
            assert!(
                matches!(frame, Frame::Event(AgentEvent::Other { kind }) if kind == "extension_ui_request"),
                "a fire-and-forget request must stay untyped: {frame:?}"
            );
        }
    }

    #[test]
    fn encode_ui_response_writes_the_exact_protocol_line() {
        // One line, LF terminated, no allocated request id: pi answers nothing
        // and the id is the one pi chose (S2).
        assert_eq!(
            encode_ui_response("3f9c-uuid", &UiAnswer::Value { value: "Allow".into() }),
            "{\"type\":\"extension_ui_response\",\"id\":\"3f9c-uuid\",\"value\":\"Allow\"}\n"
        );
        assert_eq!(
            encode_ui_response("3f9c-uuid", &UiAnswer::Confirmed { confirmed: true }),
            "{\"type\":\"extension_ui_response\",\"id\":\"3f9c-uuid\",\"confirmed\":true}\n"
        );
        assert_eq!(
            encode_ui_response("3f9c-uuid", &UiAnswer::Cancelled { cancelled: true }),
            "{\"type\":\"extension_ui_response\",\"id\":\"3f9c-uuid\",\"cancelled\":true}\n"
        );
    }

    #[test]
    fn encode_ui_response_escapes_the_answer_and_keeps_one_line() {
        let line = encode_ui_response(
            "uuid",
            &UiAnswer::Value {
                value: "a \"quoted\"\nsecond line".into(),
            },
        );
        assert_eq!(line.matches('\n').count(), 1, "exactly one newline: {line:?}");
        let parsed: Value = serde_json::from_str(line.trim_end()).expect("valid json");
        assert_eq!(parsed["type"], "extension_ui_response");
        assert_eq!(parsed["value"], "a \"quoted\"\nsecond line");
        assert_eq!(parsed["id"], "uuid");
    }

    #[test]
    fn ui_answer_deserializes_the_three_documented_shapes() {
        assert_eq!(
            serde_json::from_value::<UiAnswer>(json!({"value": "Allow"})).unwrap(),
            UiAnswer::Value { value: "Allow".into() }
        );
        assert_eq!(
            serde_json::from_value::<UiAnswer>(json!({"confirmed": false})).unwrap(),
            UiAnswer::Confirmed { confirmed: false }
        );
        assert_eq!(
            serde_json::from_value::<UiAnswer>(json!({"cancelled": true})).unwrap(),
            UiAnswer::Cancelled { cancelled: true }
        );
        // Nothing to answer is not an answer: the shape is a closed set.
        assert!(
            serde_json::from_value::<UiAnswer>(json!({})).is_err(),
            "an empty answer must be rejected"
        );
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
            stop_reason,
            error_message,
        }) = &frames[1]
        else {
            panic!("expected turn_end, got {:?}", frames[1]);
        };
        assert_eq!(message, &json!({"role": "assistant"}));
        assert!(tool_results.is_empty());
        // S3 of `turn-continuity.md`: a turn_end that reports no reason adds no
        // field, and the message alone is still passed through.
        assert_eq!(stop_reason, &None);
        assert_eq!(error_message, &None);
    }

    // S3 of `turn-continuity.md`: why a turn ended already travels in
    // `message.stopReason`; the codec used to drop it with the passthrough
    // message. Each fixture is one word pi sends — including one Nabla does not
    // model, which must degrade to a typed value instead of demoting the frame
    // to `Other` and losing the turn.
    #[test]
    fn turn_end_projects_the_stop_reason_out_of_the_message() {
        for word in ["aborted", "length", "error", "stop", "a_reason_from_the_future"] {
            let mut codec = FrameCodec::new();
            let line = format!(
                concat!(
                    r#"{{"type":"turn_end","message":{{"role":"assistant","#,
                    r#""stopReason":"{}"}},"toolResults":[]}}"#,
                    "\n"
                ),
                word
            );
            let frames = codec.push_bytes(line.as_bytes());
            assert_eq!(frames.len(), 1);
            let Frame::Event(event) = &frames[0] else {
                panic!("expected turn_end for {word:?}, got {:?}", frames[0]);
            };
            assert!(
                matches!(event, AgentEvent::TurnEnd { .. }),
                "an unmodeled stop reason must not demote the frame to `Other`: {event:?}"
            );
            let wire = serde_json::to_value(event).expect("turn_end serializes");
            assert_eq!(
                wire["stopReason"],
                json!(word),
                "the reason pi sent reaches the webview verbatim"
            );
            assert_eq!(
                wire["message"],
                json!({"role": "assistant", "stopReason": word}),
                "the message still travels whole, reason included"
            );
        }
    }

    #[test]
    fn turn_end_carries_the_error_message_when_pi_sends_one() {
        let mut codec = FrameCodec::new();
        let line = concat!(
            r#"{"type":"turn_end","message":{"role":"assistant","stopReason":"error","#,
            r#""errorMessage":"provider refused the request"},"toolResults":[]}"#,
            "\n"
        );
        let frames = codec.push_bytes(line.as_bytes());
        let Frame::Event(event) = &frames[0] else {
            panic!("expected turn_end, got {:?}", frames[0]);
        };
        let wire = serde_json::to_value(event).expect("turn_end serializes");
        assert_eq!(wire["stopReason"], json!("error"));
        assert_eq!(wire["errorMessage"], json!("provider refused the request"));
    }

    #[test]
    fn turn_end_without_a_reported_reason_omits_both_fields() {
        // A turn that ended on its own announces nothing (S3): an absent field
        // says "pi did not report a reason", and it is never `null` on the wire.
        let mut codec = FrameCodec::new();
        let frames = codec.push_bytes(
            b"{\"type\":\"turn_end\",\"message\":{\"role\":\"assistant\"},\"toolResults\":[]}\n",
        );
        let Frame::Event(event) = &frames[0] else {
            panic!("expected turn_end, got {:?}", frames[0]);
        };
        let wire = serde_json::to_value(event).expect("turn_end serializes");
        assert!(
            wire.get("stopReason").is_none() && wire.get("errorMessage").is_none(),
            "nothing is invented for a turn pi reported no reason for: {wire}"
        );
    }

    #[test]
    fn stop_reason_round_trips_every_documented_word() {
        for (word, reason) in [
            ("pending", StopReason::Pending),
            ("stop", StopReason::Stop),
            ("length", StopReason::Length),
            ("toolUse", StopReason::ToolUse),
            ("error", StopReason::Error),
            ("aborted", StopReason::Aborted),
            ("deferred", StopReason::Deferred),
            ("word_from_the_future", StopReason::Unknown("word_from_the_future".into())),
        ] {
            assert_eq!(StopReason::from_wire(word), reason);
            assert_eq!(reason.as_wire(), word);
            assert_eq!(
                serde_json::to_value(&reason).expect("serializes"),
                json!(word)
            );
            assert_eq!(
                serde_json::from_value::<StopReason>(json!(word)).expect("decodes"),
                reason
            );
        }
    }

    #[test]
    fn decode_thinking_level_changed_keeps_the_level() {
        // S2 of `composer-inputs.md`: pi confirms the level it actually adopted
        // (it clamps to the model), and that confirmation is the only truth the
        // effort selector renders. Before this variant the frame fell into
        // `Other` and the level was lost with its payload.
        let mut codec = FrameCodec::new();
        let chunk = concat!(
            r#"{"type":"thinking_level_changed","level":"high"}"#, "\n",
            r#"{"type":"thinking_level_changed","level":"off"}"#, "\n",
        );
        let frames = codec.push_bytes(chunk.as_bytes());
        assert_eq!(frames.len(), 2);
        let Frame::Event(high) = &frames[0] else {
            panic!("expected an event, got {:?}", frames[0]);
        };
        assert_eq!(
            high,
            &AgentEvent::ThinkingLevelChanged {
                level: "high".into()
            }
        );
        // The webview mirrors this exact shape (R4), so pin it here.
        assert_eq!(
            serde_json::to_value(high).unwrap(),
            json!({"type": "thinking_level_changed", "level": "high"})
        );
        let Frame::Event(off) = &frames[1] else {
            panic!("expected an event, got {:?}", frames[1]);
        };
        assert_eq!(
            serde_json::to_value(off).unwrap(),
            json!({"type": "thinking_level_changed", "level": "off"})
        );
    }

    #[test]
    fn decode_thinking_level_changed_keeps_a_word_from_a_newer_pi() {
        // A level pi introduces later must not demote the whole frame to `Other`
        // and leave the selector stale: the field is a string, so it crosses.
        let mut codec = FrameCodec::new();
        let frames = codec
            .push_bytes(b"{\"type\":\"thinking_level_changed\",\"level\":\"ultra\"}\n");
        assert!(matches!(
            &frames[0],
            Frame::Event(AgentEvent::ThinkingLevelChanged { level }) if level == "ultra"
        ));
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
