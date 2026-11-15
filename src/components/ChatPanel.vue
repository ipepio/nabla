<script setup lang="ts">
// Tracer-bullet chat (spec S6, task R4): one pi session per selected workspace,
// streaming assistant text and tool activity into a minimal transcript.
// Everything pi sends arrives through the contracts seam (`subscribeAgentEvents`
// → `agent-event` envelope), never from a view-level Tauri import.
//
// S1/S2: this rewrite changes presentation only. The subscription, the delta
// folding, the tool-state machine, the status mapping and the send/stop paths
// are the same code the earlier panel had; the shell around them is new:
// an empty state before the first turn and a header + Chat/Linaje tabs after.
import type { Component } from "vue";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import {
  CheckIcon,
  CircleAlertIcon,
  LoaderIcon,
  PanelRightOpenIcon,
  XIcon,
} from "@lucide/vue";
import ChatEmptyState from "@/components/chat/ChatEmptyState.vue";
import ThemeToggle from "@/components/chat/ThemeToggle.vue";
import ChatHeader from "@/components/chat/ChatHeader.vue";
import Composer from "@/components/chat/Composer.vue";
import MessageBubble from "@/components/chat/MessageBubble.vue";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { AgentEvent, RuntimeStatus } from "@/contracts/agent-events";
import {
  commandErrorMessage,
  getSettings,
  listModels,
  saveSettings,
  sendPrompt,
  sessionStatus,
  stopSession,
  subscribeAgentEvents,
} from "@/contracts/commands";
import type { AppSettings, ModelInfo, SessionDto, WorkspaceInfo } from "@/contracts/types";
import { useAppearance } from "@/lib/appearance";
import {
  activeSession,
  newSession,
  sessionGeneration,
} from "@/lib/session-store";
import { isDarkMode, type ThemeMode } from "@/lib/theme";

/** The drawer owns the workspace list; the active one arrives already chosen. */
const props = defineProps<{
  workspace: WorkspaceInfo | null;
  /** Right tools drawer visibility (App.vue owns the state). */
  toolsOpen: boolean;
}>();

const emit = defineEmits<{ (e: "toggleTools"): void }>();

/** One rendered turn. Assistant text grows as deltas arrive. */
interface ChatMessage {
  role: "user" | "assistant";
  text: string;
  /** `HH:MM` the turn was added, shown under user bubbles (S2). */
  at: string;
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

/**
 * N1 of `sidebar-rework.md`: the active session now lives in the shared store
 * (`src/lib/session-store.ts`) because the drawer creates and opens sessions
 * too. This panel still owns everything about *rendering* it.
 */
const currentSession = computed<SessionDto | null>(() => {
  const active = activeSession.value;
  if (active === null || props.workspace === null) return null;
  // A session belongs to one workspace: switching workspaces must never route a
  // prompt to the previous workspace's process, so the old session stops being
  // "current" here and the next send starts a fresh one.
  return active.workspaceId === props.workspace.id ? active : null;
});

/**
 * Creating, opening or clearing the active session replaces what the transcript
 * shows, so the per-session view state resets while the store keeps the
 * identity. `flush: "sync"` keeps the reset ahead of the turn the composer is
 * about to echo into `messages`.
 *
 * A past session's content lives in pi's JSONL file and no command reads it
 * yet, so opening an older session starts the view empty (documented
 * follow-up).
 */
function resetTranscript() {
  messages.value = [];
  tools.value = [];
  status.value = null;
  statusError.value = null;
  exitCode.value = null;
  error.value = null;
  prompt.value = "";
}

watch(sessionGeneration, resetTranscript, { flush: "sync" });

// Unsubscribe handle for the `agent-event` listener, kept so unmounting does
// not leave a handler reading into a disposed component.
let unsubscribe: (() => void) | null = null;

// S5/S7: the model selector's data. `models` is whatever Rust reports (never a
// hardcoded list here) and `settings` holds the persisted choice.
const models = ref<ModelInfo[]>([]);
const settings = ref<AppSettings | null>(null);
const savingModel = ref(false);

// S2/S4 of `theme-modes-and-resizable-drawer.md`: the appearance state lives in
// one module so the sidebar, the empty state and this panel all agree on the
// mode (the logo variant follows it); `src/lib/theme.ts` stays the only module
// that applies the palette.
const { mode: theme, isDark, setMode } = useAppearance();

/** Selected model id, or `""` until settings load. */
const modelId = computed(() => settings.value?.model ?? "");

/** S1: no turn has been rendered yet, so the empty state owns the panel. */
const isEmptyState = computed(
  () => messages.value.length === 0 && tools.value.length === 0 && error.value === null,
);

const canSend = computed(
  () => props.workspace !== null && prompt.value.trim() !== "" && !sending.value && !starting.value,
);

/** Provider reported by settings, used by the empty-state chip (S1). */
const providerName = computed(() => settings.value?.provider ?? "pi");

/** Header title: the active space's name (S2). */
const workspaceName = computed(() => props.workspace?.name ?? "Nabla");

/** Footer meta under the composer (S2): user turns and tool steps so far. */
const turns = computed(
  () => messages.value.filter((message) => message.role === "user").length,
);
const steps = computed(() => tools.value.length);

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

/** Semantic tone for the status alert; success/warning are the new tokens (S4). */
function statusAlertClass(type: StatusAlert["type"]): string {
  switch (type) {
    case "success":
      return "text-success";
    case "warning":
      return "text-warning";
    default:
      return "";
  }
}

/** Local clock as `HH:MM`; the bubble only shows the hour a turn was added. */
function timeLabel(): string {
  const now = new Date();
  const hours = String(now.getHours()).padStart(2, "0");
  const minutes = String(now.getMinutes()).padStart(2, "0");
  return `${hours}:${minutes}`;
}

function appendAssistantDelta(delta: string) {
  const last = messages.value[messages.value.length - 1];
  if (last !== undefined && last.role === "assistant") {
    last.text += delta;
  } else {
    messages.value.push({ role: "assistant", text: delta, at: timeLabel() });
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

/**
 * S1: the empty state has no "start session" control, so the first send starts
 * the session itself. N1: it starts through the shared store, so this and the
 * drawer's "New session" button are one creation path and the drawer sees the
 * session the chat is talking to. Returns the live session, or `null` when it
 * could not be started (the error is already rendered then).
 */
async function ensureSession(): Promise<SessionDto | null> {
  const existing = currentSession.value;
  if (existing !== null) return existing;
  const workspace = props.workspace;
  if (workspace === null) {
    error.value = "Select a workspace before sending a prompt.";
    return null;
  }
  starting.value = true;
  try {
    const started = await newSession(workspace.id);
    // The store bumped the generation, which reset the view; the start status is
    // the one fact that reset cannot know.
    status.value = started.status;
    return started;
  } catch (cause) {
    error.value = commandErrorMessage(cause);
    return null;
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
    // Apply the persisted mode as soon as the file is read, so the first paint
    // after startup shows the theme the human chose.
    setMode(settings.value.theme);
  } catch (cause) {
    settings.value = null;
    error.value = commandErrorMessage(cause);
  }
}

/**
 * S2: switches palette and persists the choice. The next mode is computed from
 * the current one; `system` toggles to whatever the OS is *not* showing right
 * now. The whole settings object is re-read first, so this write cannot clobber
 * a preference the drawer changed in the meantime (`save_settings` replaces the
 * file).
 */
async function handleToggleTheme() {
  const current = theme.value;
  const next: ThemeMode =
    current === "system"
      ? isDarkMode("system")
        ? "light"
        : "dark"
      : current === "dark"
        ? "light"
        : "dark";
  error.value = null;
  try {
    const stored = await getSettings();
    settings.value = await saveSettings({ ...stored, theme: next });
    setMode(next);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  }
}

/**
 * S5/S7: persists the chosen model. The whole settings file is re-read first so
 * the write cannot clobber a preference the drawer changed in the meantime
 * (`save_settings` replaces the file). The choice applies to the next spawn.
 */
async function handleModelChange(value: string) {
  if (value === "") return;
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
  const text = prompt.value.trim();
  if (text === "" || sending.value || starting.value) return;
  sending.value = true;
  error.value = null;
  try {
    const current = await ensureSession();
    if (current === null) return;
    await sendPrompt(current.workspaceId, Number(current.sessionId), text);
    // Echo the turn so the transcript reads as a conversation: pi answers with
    // deltas on `agent-event`, it does not echo the prompt back.
    messages.value.push({ role: "user", text, at: timeLabel() });
    prompt.value = "";
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    sending.value = false;
  }
}

async function handleStop() {
  const current = currentSession.value;
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
    // One global channel carries every session; only the active session of the
    // workspace on screen is this panel's business.
    if (envelope.sessionId !== currentSession.value?.sessionId) return;
    handleEvent(envelope.event);
  });
});

onUnmounted(() => {
  unsubscribe?.();
  unsubscribe = null;
});
</script>

<template>
  <div class="relative flex h-full min-h-0 flex-col">
    <!-- Corner cluster for the no-conversation state: the header (which carries
         the same controls) does not exist yet, so both switches live here until
         the first turn. Tools toggle hides while the drawer is open.
         -->
    <div v-if="isEmptyState" class="absolute top-2 right-2 z-10 flex items-center gap-1">
      <ThemeToggle :is-dark="isDark" @toggle="handleToggleTheme" />
      <Button
        v-if="!toolsOpen"
        variant="ghost"
        size="icon-sm"
        aria-label="Expand tools panel"
        :aria-pressed="toolsOpen"
        @click="emit('toggleTools')"
      >
        <PanelRightOpenIcon />
      </Button>
    </div>

    <!-- S1: no conversation yet — brand mark, chips and composer, no header. -->
    <ChatEmptyState
      v-if="isEmptyState"
      :provider="providerName"
      :hint="
        workspace === null
          ? 'No workspace selected — add or pick one in the sidebar.'
          : null
      "
    >
      <template #composer>
        <Composer
          v-model="prompt"
          :model-id="modelId"
          :models="models"
          :workspace-name="workspace?.name ?? 'no workspace'"
          :streaming="sending"
          :disabled="workspace === null"
          :can-send="canSend"
          :model-disabled="savingModel || models.length === 0"
          :can-stop="currentSession !== null && !stopping"
          @update:model-id="handleModelChange"
          @send="handleSend"
          @stop="handleStop"
        />
      </template>
    </ChatEmptyState>

    <!-- S2: conversation state — header, Chat/Linaje tabs, transcript, composer. -->
    <template v-else>
      <ChatHeader
        :title="workspaceName"
        mode="Standard mode"
        :tools-open="toolsOpen"
        :is-dark="isDark"
        @toggle-tools="emit('toggleTools')"
        @toggle-theme="handleToggleTheme"
      />

      <Tabs default-value="chat" class="min-h-0 flex-1 gap-0">
        <div class="border-b border-border px-4">
          <TabsList variant="line" class="h-9">
            <TabsTrigger value="chat">Chat</TabsTrigger>
            <TabsTrigger value="linaje">Linaje</TabsTrigger>
          </TabsList>
        </div>

        <TabsContent value="chat" class="flex min-h-0 flex-1 flex-col">
          <!-- Transcript: the only scrolling region of the panel. -->
          <ScrollArea class="min-h-0 flex-1">
            <div class="flex flex-col gap-4 p-4">
              <Alert
                v-if="statusAlert"
                :variant="statusAlert.type === 'error' ? 'destructive' : 'default'"
              >
                <AlertDescription :class="statusAlertClass(statusAlert.type)">
                  {{ statusAlert.text }}
                </AlertDescription>
              </Alert>

              <!-- S2: failures render inside the transcript flow, not as a
                   floating toast. -->
              <div v-if="error" class="relative rounded-md border border-border p-3">
                <div class="flex items-center gap-1.5 pr-6">
                  <span
                    class="size-1.5 shrink-0 rounded-full bg-destructive"
                    aria-hidden="true"
                  />
                  <span class="text-xs font-semibold">Error</span>
                </div>
                <p class="mt-1 text-sm text-muted-foreground">{{ error }}</p>
                <div class="mt-2 flex items-center justify-end">
                  <span
                    class="text-[10px] font-medium tracking-wider text-muted-foreground uppercase"
                  >
                    TURN_FAILED
                  </span>
                </div>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  class="absolute top-1.5 right-1.5"
                  aria-label="Dismiss"
                  @click="error = null"
                >
                  <XIcon />
                </Button>
              </div>

              <MessageBubble
                v-for="(message, index) in messages"
                :key="index"
                :role="message.role"
                :text="message.text"
                :at="message.at"
              />

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

          <!-- Composer, pinned under the transcript, plus the turns/steps meta. -->
          <div class="border-t border-border px-4 py-3">
            <Composer
              v-model="prompt"
              :model-id="modelId"
              :models="models"
              :workspace-name="workspace?.name ?? 'no workspace'"
              :streaming="sending"
              :disabled="workspace === null"
              :can-send="canSend"
              :model-disabled="savingModel || models.length === 0"
              :can-stop="currentSession !== null && !stopping"
              @update:model-id="handleModelChange"
              @send="handleSend"
              @stop="handleStop"
            />
            <p class="mt-2 text-center text-xs text-muted-foreground">
              {{ turns }} turns · {{ steps }} steps
            </p>
          </div>
        </TabsContent>

        <TabsContent
          value="linaje"
          class="flex min-h-0 flex-1 items-center justify-center"
        >
          <p class="text-center text-sm text-muted-foreground">
            Activity trace will appear here.
          </p>
        </TabsContent>
      </Tabs>
    </template>
  </div>
</template>
