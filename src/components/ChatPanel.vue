<script setup lang="ts">
// Tracer-bullet chat (spec S6, task R4): one pi session per selected workspace,
// streaming assistant text and tool activity into a minimal transcript.
// Everything pi sends arrives through the contracts seam, never from a
// view-level Tauri import.
//
// Tasks L3/L4 of `session-lifecycle.md`: this panel no longer owns the stream.
// `src/lib/session-store.ts` keeps one buffer per session and one subscription
// that routes by session id, so the panel is a *renderer* of the active
// session's buffer. Switching sessions (S4) is therefore a pure view change:
// nothing here starts, stops or resets a process, and the previous session keeps
// running and keeps accumulating events off screen.
import type { Component } from "vue";
import { computed, onMounted, ref } from "vue";
import {
  CheckIcon,
  CircleAlertIcon,
  LoaderIcon,
  PanelRightOpenIcon,
  WrenchIcon,
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
import {
  commandErrorMessage,
  getSettings,
  listModels,
  saveSettings,
  sendPrompt,
  sessionStatus,
  stopSession,
} from "@/contracts/commands";
import type {
  AppSettings,
  ModelInfo,
  SessionDto,
  TranscriptRole,
  WorkspaceInfo,
} from "@/contracts/types";
import { useAppearance } from "@/lib/appearance";
import {
  activeSession,
  appendUserTurn,
  ensureSessionEvents,
  isTranscriptLoading,
  newSession,
  recordSessionStatus,
  sessionBuffer,
  type SessionBuffer,
  type ToolEntry,
} from "@/lib/session-store";
import { isDarkMode, type ThemeMode } from "@/lib/theme";

/** The drawer owns the workspace list; the active one arrives already chosen. */
const props = defineProps<{
  workspace: WorkspaceInfo | null;
  /** Right tools drawer visibility (App.vue owns the state). */
  toolsOpen: boolean;
}>();

const emit = defineEmits<{ (e: "toggleTools"): void }>();

interface StatusAlert {
  type: "info" | "success" | "warning" | "error";
  text: string;
}

const prompt = ref("");

const starting = ref(false);
const sending = ref(false);
const stopping = ref(false);
/** Last command failure of this panel; the session's own diagnosis lives in
 * its buffer, so switching sessions cannot carry one over the other. */
const panelError = ref<string | null>(null);

/**
 * N1 of `sidebar-rework.md`: the active session lives in the shared store
 * (`src/lib/session-store.ts`) because the drawer creates and selects sessions
 * too. L3 adds the other half: the store also owns what the session has said, so
 * this panel renders a buffer it does not own.
 */
const currentSession = computed<SessionDto | null>(() => {
  const active = activeSession.value;
  if (active === null || props.workspace === null) return null;
  // A session belongs to one workspace: switching workspaces must never route a
  // prompt to the previous workspace's process, so the old session stops being
  // "current" here and the next send starts a fresh one. Its process is not
  // touched — it keeps running off screen (S4).
  return active.workspaceId === props.workspace.id ? active : null;
});

/**
 * The buffer of the session on screen. Everything below — transcript, streaming
 * tail, tools, status — is read from here, which is why selecting another
 * session is instant and why a session that was off screen still shows its
 * history when it comes back (S4).
 */
const buffer = computed<SessionBuffer | null>(() =>
  sessionBuffer(currentSession.value?.sessionId ?? null),
);
const transcript = computed(() => buffer.value?.transcript ?? []);
const tools = computed<ToolEntry[]>(() => buffer.value?.tools ?? []);
const streamingText = computed(() => buffer.value?.streamingText ?? "");
const transcriptLoading = computed(() =>
  isTranscriptLoading(currentSession.value?.sessionId ?? null),
);

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
const isEmptyState = computed(() => {
  // S3: a session whose history is being read shows the conversation shell, so
  // the panel does not flash the brand empty state on the way in.
  if (transcriptLoading.value) return false;
  const current = buffer.value;
  if (current === null) return true;
  return (
    current.transcript.length === 0 &&
    current.streamingText === "" &&
    current.tools.length === 0 &&
    errorText.value === null
  );
});

const canSend = computed(
  () => props.workspace !== null && prompt.value.trim() !== "" && !sending.value && !starting.value,
);

/** Provider reported by settings, used by the empty-state chip (S1). */
const providerName = computed(() => settings.value?.provider ?? "pi");

/** Header title: the active space's name (S2). */
const workspaceName = computed(() => props.workspace?.name ?? "Nabla");

/** Footer meta under the composer (S2): user turns and tool steps so far. */
const turns = computed(
  () => transcript.value.filter((message) => message.role === "user").length,
);
const steps = computed(() => tools.value.length);

/**
 * The dismissible failure card. Command failures come from this panel; a
 * session-level error comes from its buffer, except while the process crashed —
 * the status alert below already names the stderr tail there.
 */
const errorText = computed<string | null>(() => {
  if (panelError.value !== null) return panelError.value;
  const current = buffer.value;
  if (current === null || current.status === "crashed") return null;
  return current.error;
});

/** Dismisses whichever failure is on screen, panel-owned or session-owned. */
function dismissError() {
  if (panelError.value !== null) {
    panelError.value = null;
    return;
  }
  if (buffer.value !== null) buffer.value.error = null;
}

/** Human-readable lifecycle state, the same five states the runtime models. */
const statusAlert = computed<StatusAlert | null>(() => {
  const current = buffer.value?.status ?? null;
  if (current === null) return null;
  const exit = buffer.value?.exitCode ?? null;
  const runtimeError = buffer.value?.error ?? null;
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
          exit === null ? "pi is stopped." : `pi is stopped (exit code ${exit}).`,
      };
    case "crashed":
      return {
        type: "error",
        text: runtimeError ? `pi crashed: ${runtimeError}` : "pi crashed.",
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

/**
 * Local clock as `HH:MM`. Buffer timestamps are pi's own string (or the ISO
 * stamp the store writes for a local turn); an unparseable one renders nothing
 * rather than "Invalid Date".
 */
function timeLabel(at: string | null): string {
  if (at === null || at === "") return "";
  const then = new Date(at);
  if (Number.isNaN(then.getTime())) return "";
  const hours = String(then.getHours()).padStart(2, "0");
  const minutes = String(then.getMinutes()).padStart(2, "0");
  return `${hours}:${minutes}`;
}

/**
 * `MessageBubble` knows the two speaking roles; a `tool` line of a recovered
 * transcript renders as the muted note below instead of a bubble.
 */
function bubbleRole(role: TranscriptRole): "user" | "assistant" {
  return role === "user" ? "user" : "assistant";
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
    panelError.value = "Select a workspace before sending a prompt.";
    return null;
  }
  starting.value = true;
  try {
    // The store creates the buffer, marks it live and makes it active; the panel
    // then simply renders it.
    return await newSession(workspace.id);
  } catch (cause) {
    panelError.value = commandErrorMessage(cause);
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
    panelError.value = commandErrorMessage(cause);
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
    panelError.value = commandErrorMessage(cause);
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
  panelError.value = null;
  try {
    const stored = await getSettings();
    settings.value = await saveSettings({ ...stored, theme: next });
    setMode(next);
  } catch (cause) {
    panelError.value = commandErrorMessage(cause);
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
  panelError.value = null;
  try {
    const current = await getSettings();
    settings.value = await saveSettings({ ...current, model: value });
  } catch (cause) {
    panelError.value = commandErrorMessage(cause);
  } finally {
    savingModel.value = false;
  }
}

async function handleSend() {
  const text = prompt.value.trim();
  if (text === "" || sending.value || starting.value) return;
  sending.value = true;
  panelError.value = null;
  try {
    const current = await ensureSession();
    if (current === null) return;
    await sendPrompt(current.workspaceId, Number(current.sessionId), text);
    // Echo the turn so the transcript reads as a conversation: pi answers with
    // deltas on `agent-event`, it does not echo the prompt back. The turn lands
    // in that session's buffer, not in whatever is on screen.
    appendUserTurn(current.sessionId, text);
    prompt.value = "";
  } catch (cause) {
    panelError.value = commandErrorMessage(cause);
  } finally {
    sending.value = false;
  }
}

/**
 * Stops the active session. This is the only path that stops a process —
 * changing sessions never does (S4), and the button only touches the session on
 * screen.
 */
async function handleStop() {
  const current = currentSession.value;
  if (current === null) return;
  stopping.value = true;
  panelError.value = null;
  try {
    await stopSession(current.workspaceId, Number(current.sessionId));
    // Confirm with the runtime instead of assuming the transition landed.
    const snapshot = await sessionStatus(
      current.workspaceId,
      Number(current.sessionId),
    );
    recordSessionStatus(current.sessionId, snapshot.status, snapshot.error);
  } catch (cause) {
    panelError.value = commandErrorMessage(cause);
  } finally {
    stopping.value = false;
  }
}

onMounted(() => {
  // Presentation-only additions (S5): the header needs the active space's name
  // (already a prop) plus the model options and the persisted choice.
  void loadModelOptions();
  void loadSettings();
  // L3/S4: the store owns the single `agent-event` subscription and buffers
  // every session, so this panel only asks for it to be installed. It is never
  // torn down with the panel: a background session must keep receiving events.
  void ensureSessionEvents();
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
              <div v-if="errorText" class="relative rounded-md border border-border p-3">
                <div class="flex items-center gap-1.5 pr-6">
                  <span
                    class="size-1.5 shrink-0 rounded-full bg-destructive"
                    aria-hidden="true"
                  />
                  <span class="text-xs font-semibold">Error</span>
                </div>
                <p class="mt-1 text-sm text-muted-foreground">{{ errorText }}</p>
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
                  @click="dismissError"
                >
                  <XIcon />
                </Button>
              </div>

              <!-- S3: the session's own history is being read from pi's file. -->
              <p
                v-if="transcriptLoading && transcript.length === 0"
                class="text-sm text-muted-foreground"
              >
                Loading conversation…
              </p>

              <!-- L3/S4: the active session's buffer, which kept filling while
                   this session was off screen. -->
              <template v-for="(message, index) in transcript" :key="index">
                <div
                  v-if="message.role === 'tool'"
                  class="flex items-start gap-2 text-xs text-muted-foreground"
                >
                  <WrenchIcon class="mt-0.5 size-3 shrink-0" aria-hidden="true" />
                  <span class="min-w-0 whitespace-pre-wrap">{{ message.text }}</span>
                </div>
                <MessageBubble
                  v-else
                  :role="bubbleRole(message.role)"
                  :text="message.text"
                  :at="timeLabel(message.at)"
                />
              </template>

              <!-- The turn still streaming: it becomes a transcript entry when
                   pi closes the message. -->
              <MessageBubble
                v-if="streamingText !== ''"
                role="assistant"
                :text="streamingText"
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
