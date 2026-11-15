<script setup lang="ts">
// Tracer-bullet chat (spec S6, task R4): one pi session per selected workspace,
// streaming assistant text and tool activity into a minimal transcript.
// Everything pi sends arrives through the contracts seam (`subscribeAgentEvents`
// → `agent-event` envelope), never from a view-level Tauri import.
import { computed, onMounted, onUnmounted, ref } from "vue";
// Imported locally: main.ts installs the Vuetify plugin without a component
// registry, so each view registers what it renders (same pattern as App.vue).
import {
  VAlert,
  VBtn,
  VCard,
  VCardText,
  VCardTitle,
  VChip,
  VCol,
  VRow,
  VSelect,
  VTextField,
} from "vuetify/components";
import type { AgentEvent, RuntimeStatus } from "../contracts/agent-events";
import {
  commandErrorMessage,
  sendPrompt,
  sessionStatus,
  startSession,
  stopSession,
  subscribeAgentEvents,
} from "../contracts/commands";
import type { SessionDto, WorkspaceInfo } from "../contracts/types";

defineProps<{ workspaces: WorkspaceInfo[] }>();

/** One rendered turn. Assistant text grows as deltas arrive. */
interface ChatMessage {
  role: "user" | "assistant";
  text: string;
}

/** One tool call shown as a compact timeline chip, keyed by its wire id. */
interface ToolEntry {
  toolCallId: string;
  toolName: string;
  state: "running" | "done" | "error";
}

interface StatusAlert {
  type: "info" | "success" | "warning" | "error";
  text: string;
}

const selectedWorkspaceId = ref<number | null>(null);
const session = ref<SessionDto | null>(null);
const prompt = ref("");
const messages = ref<ChatMessage[]>([]);
const tools = ref<ToolEntry[]>([]);
const status = ref<RuntimeStatus | null>(null);
const statusError = ref<string | null>(null);
const exitCode = ref<number | null>(null);

const starting = ref(false);
const sending = ref(false);
const stopping = ref(false);
const error = ref<string | null>(null);

// Unsubscribe handle for the `agent-event` listener, kept so unmounting does
// not leave a handler reading into a disposed component.
let unsubscribe: (() => void) | null = null;

const canSend = computed(
  () => session.value !== null && prompt.value.trim() !== "" && !sending.value,
);

/** Human-readable lifecycle state, the same five states the runtime models. */
const statusAlert = computed<StatusAlert | null>(() => {
  const current = status.value;
  if (current === null) return null;
  switch (current) {
    case "spawning":
      return { type: "info", text: "pi is starting…" };
    case "ready":
      return { type: "success", text: "pi is ready." };
    case "running":
      return { type: "info", text: "pi is working…" };
    case "stopped":
      return {
        type: "info",
        text:
          exitCode.value === null
            ? "pi is stopped."
            : `pi is stopped (exit code ${exitCode.value}).`,
      };
    case "crashed":
      return {
        type: "error",
        text: statusError.value
          ? `pi crashed: ${statusError.value}`
          : "pi crashed.",
      };
  }
});

function appendAssistantDelta(delta: string) {
  const last = messages.value[messages.value.length - 1];
  if (last !== undefined && last.role === "assistant") {
    last.text += delta;
  } else {
    messages.value.push({ role: "assistant", text: delta });
  }
}

function findTool(toolCallId: string): ToolEntry | undefined {
  return tools.value.find((entry) => entry.toolCallId === toolCallId);
}

/** Applies one forwarded pi event to the transcript. */
function handleEvent(event: AgentEvent) {
  switch (event.type) {
    case "message_update": {
      const slice = event.assistantMessageEvent;
      // Only grown text is rendered; thinking slices and empty markers carry
      // nothing this view shows yet.
      if (slice.type === "text_delta" && slice.delta !== undefined && slice.delta !== "") {
        appendAssistantDelta(slice.delta);
      }
      return;
    }
    case "tool_execution_start":
      tools.value.push({
        toolCallId: event.toolCallId,
        toolName: event.toolName,
        state: "running",
      });
      return;
    case "tool_execution_update": {
      const entry = findTool(event.toolCallId);
      if (entry === undefined) {
        tools.value.push({
          toolCallId: event.toolCallId,
          toolName: event.toolName,
          state: "running",
        });
      } else {
        entry.state = "running";
      }
      return;
    }
    case "tool_execution_end": {
      const entry = findTool(event.toolCallId);
      if (entry === undefined) {
        tools.value.push({
          toolCallId: event.toolCallId,
          toolName: event.toolName,
          state: event.isError ? "error" : "done",
        });
      } else {
        entry.state = event.isError ? "error" : "done";
      }
      return;
    }
    case "process_status":
      status.value = event.status;
      statusError.value = event.error ?? null;
      return;
    case "process_exit":
      exitCode.value = event.code ?? null;
      return;
    default:
      // agent_start/agent_end/turn_start/turn_end and unmodelled pi events add
      // nothing to this view yet; the stream keeps flowing past them.
      return;
  }
}

function toolIcon(state: ToolEntry["state"]): string {
  switch (state) {
    case "running":
      return "mdi-progress-wrench";
    case "done":
      return "mdi-check";
    case "error":
      return "mdi-alert-circle-outline";
  }
}

function toolColor(state: ToolEntry["state"]): string {
  switch (state) {
    case "running":
      return "info";
    case "done":
      return "success";
    case "error":
      return "error";
  }
}

function toolStateLabel(state: ToolEntry["state"]): string {
  switch (state) {
    case "running":
      return "running";
    case "done":
      return "done";
    case "error":
      return "failed";
  }
}

async function handleStart() {
  const workspaceId = selectedWorkspaceId.value;
  if (workspaceId === null) {
    error.value = "Select a workspace before starting a session.";
    return;
  }
  starting.value = true;
  error.value = null;
  try {
    const started = await startSession(workspaceId);
    session.value = started;
    messages.value = [];
    tools.value = [];
    status.value = started.status;
    statusError.value = null;
    exitCode.value = null;
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    starting.value = false;
  }
}

async function handleSend() {
  const current = session.value;
  const text = prompt.value.trim();
  if (current === null || text === "" || sending.value) return;
  sending.value = true;
  error.value = null;
  try {
    await sendPrompt(current.workspaceId, Number(current.sessionId), text);
    // Echo the turn so the transcript reads as a conversation: pi answers with
    // deltas on `agent-event`, it does not echo the prompt back.
    messages.value.push({ role: "user", text });
    prompt.value = "";
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    sending.value = false;
  }
}

async function handleStop() {
  const current = session.value;
  if (current === null) return;
  stopping.value = true;
  error.value = null;
  try {
    await stopSession(current.workspaceId, Number(current.sessionId));
    // Confirm with the runtime instead of assuming the transition landed.
    const snapshot = await sessionStatus(
      current.workspaceId,
      Number(current.sessionId),
    );
    status.value = snapshot.status;
    statusError.value = snapshot.error;
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    stopping.value = false;
  }
}

onMounted(async () => {
  unsubscribe = await subscribeAgentEvents((envelope) => {
    const current = session.value;
    // One global channel carries every session; only this panel's session is
    // its business.
    if (current === null || envelope.sessionId !== current.sessionId) return;
    handleEvent(envelope.event);
  });
});

onUnmounted(() => {
  unsubscribe?.();
  unsubscribe = null;
});
</script>

<template>
  <v-card elevation="2" rounded="lg">
    <v-card-title class="text-subtitle-1">Chat</v-card-title>
    <v-card-text>
      <v-alert
        v-if="error"
        class="mb-4"
        type="error"
        variant="tonal"
        closable
        @click:close="error = null"
      >
        {{ error }}
      </v-alert>

      <v-row density="comfortable" align="center">
        <v-col cols="12" md="6">
          <v-select
            v-model="selectedWorkspaceId"
            :items="workspaces"
            item-title="name"
            item-value="id"
            label="Workspace"
            variant="outlined"
            density="comfortable"
            hide-details
            :disabled="session !== null || starting"
          />
        </v-col>
        <v-col cols="12" md="6" class="d-flex align-center ga-3">
          <v-btn
            color="primary"
            :loading="starting"
            :disabled="selectedWorkspaceId === null || session !== null"
            @click="handleStart"
          >
            Start session
          </v-btn>
          <span v-if="session !== null" class="text-caption text-medium-emphasis">
            session {{ session.sessionId }} · pid {{ session.pid }}
          </span>
        </v-col>
      </v-row>

      <v-alert
        v-if="statusAlert"
        class="mt-4"
        :type="statusAlert.type"
        variant="tonal"
        density="compact"
      >
        {{ statusAlert.text }}
      </v-alert>

      <div v-if="session !== null" class="chat-transcript mt-4">
        <p v-if="messages.length === 0" class="text-medium-emphasis">
          No messages yet — send a prompt below.
        </p>
        <div
          v-for="(message, index) in messages"
          :key="index"
          class="chat-message"
          :class="`chat-message--${message.role}`"
        >
          <span class="text-caption text-medium-emphasis">
            {{ message.role === "user" ? "You" : "pi" }}
          </span>
          <p class="chat-text">{{ message.text }}</p>
        </div>

        <div v-if="tools.length > 0" class="d-flex flex-wrap ga-2 mt-3">
          <v-chip
            v-for="tool in tools"
            :key="tool.toolCallId"
            size="small"
            variant="tonal"
            :color="toolColor(tool.state)"
            :prepend-icon="toolIcon(tool.state)"
          >
            {{ tool.toolName }} · {{ toolStateLabel(tool.state) }}
          </v-chip>
        </div>
      </div>

      <v-row class="mt-2" density="comfortable" align="center">
        <v-col cols="12" md="9">
          <v-text-field
            v-model="prompt"
            label="Prompt"
            placeholder="Ask pi…"
            variant="outlined"
            density="comfortable"
            hide-details
            :disabled="session === null"
            @keyup.enter="handleSend"
          />
        </v-col>
        <v-col cols="8" md="2">
          <v-btn
            block
            color="primary"
            :loading="sending"
            :disabled="!canSend"
            @click="handleSend"
          >
            Send
          </v-btn>
        </v-col>
        <v-col cols="4" md="1">
          <v-btn
            icon="mdi-stop"
            variant="text"
            color="error"
            aria-label="Stop session"
            :loading="stopping"
            :disabled="session === null"
            @click="handleStop"
          />
        </v-col>
      </v-row>
    </v-card-text>
  </v-card>
</template>

<style scoped>
.chat-transcript {
  max-height: 420px;
  overflow-y: auto;
}

.chat-message--user .chat-text {
  white-space: pre-wrap;
  opacity: 0.75;
}

.chat-message--assistant .chat-text {
  white-space: pre-wrap;
}
</style>
