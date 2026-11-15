<script setup lang="ts">
// Tracer-bullet chat (spec S6, task R4): one pi session per selected workspace,
// streaming assistant text and tool activity into a minimal transcript.
// Everything pi sends arrives through the contracts seam (`subscribeAgentEvents`
// → `agent-event` envelope), never from a view-level Tauri import.
//
// S5/S1: this rewrite changes presentation only. The subscription, the delta
// folding, the tool-state machine and the status mapping are the same code the
// Vuetify panel had.
import type { Component } from "vue";
import { computed, onMounted, onUnmounted, ref } from "vue";
import {
  CheckIcon,
  CircleAlertIcon,
  LoaderIcon,
  SquareIcon,
  XIcon,
} from "@lucide/vue";
import { Alert, AlertAction, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import type { AgentEvent, RuntimeStatus } from "@/contracts/agent-events";
import {
  commandErrorMessage,
  getSettings,
  listModels,
  saveSettings,
  sendPrompt,
  sessionStatus,
  startSession,
  stopSession,
  subscribeAgentEvents,
} from "@/contracts/commands";
import type { AppSettings, ModelInfo, SessionDto, WorkspaceInfo } from "@/contracts/types";

/** The drawer owns the workspace list; the active one arrives already chosen. */
const props = defineProps<{ workspace: WorkspaceInfo | null }>();

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

// S5/S7: the model selector's data. `models` is whatever Rust reports (never a
// hardcoded list here) and `settings` holds the persisted choice.
const models = ref<ModelInfo[]>([]);
const settings = ref<AppSettings | null>(null);
const savingModel = ref(false);

/** Selected model id, or `""` until settings load. */
const modelId = computed(() => settings.value?.model ?? "");

const canSend = computed(
  () => session.value !== null && prompt.value.trim() !== "" && !sending.value,
);

const canStart = computed(
  () => props.workspace !== null && session.value === null && !starting.value,
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

function toolIcon(state: ToolEntry["state"]): Component {
  switch (state) {
    case "running":
      return LoaderIcon;
    case "done":
      return CheckIcon;
    case "error":
      return CircleAlertIcon;
  }
}

function toolBadgeVariant(state: ToolEntry["state"]): "secondary" | "outline" | "destructive" {
  switch (state) {
    case "running":
      return "secondary";
    case "done":
      return "outline";
    case "error":
      return "destructive";
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
  const workspace = props.workspace;
  if (workspace === null) {
    error.value = "Select a workspace before starting a session.";
    return;
  }
  starting.value = true;
  error.value = null;
  try {
    const started = await startSession(workspace.id);
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

async function loadModelOptions() {
  try {
    models.value = await listModels();
  } catch (cause) {
    // A missing model list must not break the chat: the session still starts
    // with whatever the settings file already names.
    models.value = [];
    error.value = commandErrorMessage(cause);
  }
}

async function loadSettings() {
  try {
    settings.value = await getSettings();
  } catch (cause) {
    settings.value = null;
    error.value = commandErrorMessage(cause);
  }
}

/**
 * S5/S7: persists the chosen model. The whole settings file is re-read first so
 * the write cannot clobber a preference the drawer changed in the meantime
 * (`save_settings` replaces the file). The choice applies to the next spawn.
 */
async function handleModelChange(value: unknown) {
  if (typeof value !== "string" || value === "") return;
  savingModel.value = true;
  error.value = null;
  try {
    const current = await getSettings();
    settings.value = await saveSettings({ ...current, model: value });
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    savingModel.value = false;
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
  // Presentation-only additions (S5): the header needs the active space's name
  // (already a prop) plus the model options and the persisted choice.
  void loadModelOptions();
  void loadSettings();
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
  <div class="flex h-full min-h-0 flex-col">
    <!-- Chat header. The collapse control lives only in the drawer (S8), so
         this header carries the active space's name (S5), the model selector
         (S5/S7) and the session controls. -->
    <header class="flex items-center gap-3 border-b px-4 py-2">
      <div class="min-w-0 flex-1">
        <h1 class="truncate text-sm font-semibold">
          {{ workspace?.name ?? "Nabla" }}
        </h1>
        <p class="truncate text-xs text-muted-foreground">
          {{
            workspace === null
              ? "No workspace selected — add or pick one in the sidebar."
              : workspace.path
          }}
        </p>
      </div>

      <!-- Model selector (S5/S7): options come from `list_models`, the value
           from settings.json. Disabled until both are known. -->
      <Select
        :model-value="modelId"
        :disabled="savingModel || models.length === 0"
        @update:model-value="handleModelChange"
      >
        <SelectTrigger class="h-8 w-[180px] text-xs" aria-label="Model">
          <SelectValue placeholder="Model" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem v-for="model in models" :key="model.id" :value="model.id">
            {{ model.name }}
          </SelectItem>
        </SelectContent>
      </Select>

      <Badge v-if="session !== null" variant="outline" class="hidden sm:inline-flex">
        session {{ session.sessionId }} · pid {{ session.pid }}
      </Badge>
      <Button v-if="session === null" :disabled="!canStart" @click="handleStart">
        {{ starting ? "Starting…" : "Start session" }}
      </Button>
      <Button
        v-else
        variant="outline"
        :disabled="stopping"
        aria-label="Stop session"
        @click="handleStop"
      >
        <SquareIcon data-icon="inline-start" />
        {{ stopping ? "Stopping…" : "Stop" }}
      </Button>
    </header>

    <div v-if="error" class="px-4 pt-3">
      <Alert variant="destructive">
        <AlertDescription>{{ error }}</AlertDescription>
        <AlertAction>
          <Button variant="ghost" size="icon-xs" aria-label="Dismiss" @click="error = null">
            <XIcon />
          </Button>
        </AlertAction>
      </Alert>
    </div>

    <div v-if="statusAlert" class="px-4 pt-3">
      <Alert :variant="statusAlert.type === 'error' ? 'destructive' : 'default'">
        <AlertDescription>{{ statusAlert.text }}</AlertDescription>
      </Alert>
    </div>

    <!-- Transcript: the only scrolling region of the app (S1). -->
    <ScrollArea class="min-h-0 flex-1">
      <div class="flex flex-col gap-5 p-4">
        <p v-if="session === null" class="text-sm text-muted-foreground">
          Start a session to talk to pi.
        </p>
        <p v-else-if="messages.length === 0" class="text-sm text-muted-foreground">
          No messages yet — send a prompt below.
        </p>

        <div
          v-for="(message, index) in messages"
          :key="index"
          class="flex flex-col gap-1"
        >
          <span class="text-xs font-medium tracking-wide text-muted-foreground uppercase">
            {{ message.role === "user" ? "You" : "pi" }}
          </span>
          <p
            class="text-sm whitespace-pre-wrap"
            :class="message.role === 'user' ? 'text-foreground/80' : 'text-foreground'"
          >
            {{ message.text }}
          </p>
        </div>

        <div v-if="tools.length > 0" class="flex flex-wrap gap-2">
          <Badge
            v-for="tool in tools"
            :key="tool.toolCallId"
            :variant="toolBadgeVariant(tool.state)"
          >
            <component :is="toolIcon(tool.state)" />
            {{ tool.toolName }} · {{ toolStateLabel(tool.state) }}
          </Badge>
        </div>
      </div>
    </ScrollArea>

    <!-- Composer, pinned under the transcript. -->
    <footer class="flex items-end gap-2 border-t p-3">
      <Textarea
        v-model="prompt"
        aria-label="Prompt"
        placeholder="Ask pi…"
        class="max-h-40 min-h-16 flex-1 resize-none"
        :disabled="session === null"
        @keydown.enter.exact.prevent="handleSend"
      />
      <Button :disabled="!canSend" @click="handleSend">
        {{ sending ? "Sending…" : "Send" }}
      </Button>
      <Button
        variant="outline"
        size="icon"
        aria-label="Stop session"
        :disabled="session === null || stopping"
        @click="handleStop"
      >
        <SquareIcon />
      </Button>
    </footer>
  </div>
</template>
