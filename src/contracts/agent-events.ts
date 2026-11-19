// TS mirror of the pi RPC event contract (spec S6, task R4).
// Source of truth: `src-tauri/src/rpc/mod.rs`. Every type below mirrors that
// module field for field, with the names serde actually serializes (camelCase
// struct fields, snake_case enum tags). The mirror rule is one-way: the Rust
// side fixes the contract and this file follows it, never the reverse
// (odd/tasks/pi-runtime.md, L3).
//
// Transport: lib.rs emits every forwarded event on the Tauri channel
// `agent-event` with the `{ sessionId, event }` envelope below (S6).

/**
 * Lifecycle of one pi child process, owned and supervised by the Nabla host
 * (spec S1, S4). Mirrors `RuntimeStatus` — `spawning -> ready -> running ->
 * stopped | crashed`.
 */
export type RuntimeStatus =
  | "spawning"
  | "ready"
  | "running"
  | "stopped"
  | "crashed";

/**
 * One progressive slice of a streamed message. Mirrors `AssistantMessageEvent`:
 * `type` is pi's own wire discriminator ("text_delta" | "thinking_delta" |
 * "start" | "done" | "error", kept verbatim), `delta` is absent on non-delta
 * slices.
 */
export interface AssistantMessageEvent {
  type: string;
  delta?: string;
  contentIndex?: number;
}

/**
 * A streamed pi event, in the exact shape pi sends it. Mirrors the `AgentEvent`
 * enum: the `type` tag is snake_case, fields are camelCase, and a frame Nabla
 * does not model yet survives as `other` carrying its wire `type` in `kind`.
 *
 * `unknown` stands in for serde's `serde_json::Value` fields: their shape is
 * pi's, not ours, so they are passed through and narrowed at the use site.
 */
export type AgentEvent =
  | { type: "agent_start" }
  | { type: "agent_end"; messages: unknown[] }
  | {
      type: "message_update";
      /**
       * Stamped by the session process when Nabla forwards the event; absent
       * while decoding a raw pi frame. The envelope's `sessionId` is the key a
       * view routes on.
       */
      sessionId?: string;
      message: unknown;
      assistantMessageEvent: AssistantMessageEvent;
    }
  | {
      type: "tool_execution_start";
      toolCallId: string;
      toolName: string;
      args: unknown;
    }
  | {
      type: "tool_execution_update";
      toolCallId: string;
      toolName: string;
      args: unknown;
      partialResult: unknown;
    }
  | {
      type: "tool_execution_end";
      toolCallId: string;
      toolName: string;
      result: unknown;
      isError: boolean;
    }
  | { type: "turn_start" }
  | { type: "turn_end"; message: unknown; toolResults: unknown[] }
  | {
      /**
       * S2 of `agent-question-bridge.md`: an extension is asking the human a
       * question and its tool call blocks until it is answered. Mirrors
       * `AgentEvent::ExtensionUiRequest`.
       *
       * `method` is pi's wire discriminator — `"select" | "confirm" | "input" |
       * "editor"` — and each method fills its own field: `options` for select,
       * `message` for confirm, `placeholder` for input. Fire-and-forget requests
       * (`notify`, `setStatus`, `setWidget`, `setTitle`, `set_editor_text`) share
       * the wire type but stay `other`: they never wait for an answer, so they
       * must never become a card nobody can close. pi's optional `timeout` is
       * deliberately not modeled either: Nabla never sets one (S4).
       */
      type: "extension_ui_request";
      /** pi's request id; the answer echoes it verbatim. */
      id: string;
      method: string;
      title?: string;
      options?: string[];
      message?: string;
      placeholder?: string;
    }
  | {
      /** Synthetic Nabla event: a lifecycle transition (S4). */
      type: "process_status";
      status: RuntimeStatus;
      /** Stderr tail; present only when the process died. */
      error?: string;
    }
  | {
      /** Synthetic Nabla event: the child exited (S4). */
      type: "process_exit";
      code?: number;
      stderrTail: string[];
    }
  | { type: "other"; kind: string };

/**
 * Payload of the `agent-event` Tauri event. Mirrors `AgentEventEnvelope` in
 * `src-tauri/src/commands.rs`: `sessionId` is the session index row id in
 * decimal form, the same key the runtime registers the process under.
 */
export interface AgentEventEnvelope {
  sessionId: string;
  event: AgentEvent;
}
