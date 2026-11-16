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
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
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
  saveSettings,
  sendPrompt,
  sessionStatus,
  stopSession,
} from "@/contracts/commands";
import type {
  AppSettings,
  SessionDto,
  TranscriptRole,
  WorkspaceInfo,
} from "@/contracts/types";
import { useAppearance } from "@/lib/appearance";
import { t } from "@/lib/i18n";
import {
  loadProviderModels,
  providerModels,
  providerModelsError,
} from "@/lib/provider-models";
import {
  activeSession,
  appendUserTurn,
  ensureSessionEvents,
  isTranscriptLoading,
  newSession,
  recordSessionStatus,
  sessionBuffer,
  sessionGeneration,
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

/**
 * Scroll behaviour of the transcript: the view follows the bottom only while the
 * human is already there. Reading the middle of a conversation must survive the
 * tokens that keep streaming in (and any other growth of the transcript), which
 * is why "at the bottom" is a state this panel tracks instead of an assumption.
 */
const BOTTOM_SLACK_PX = 48;
/** The human is at (or near) the end, so new content may move the view. */
const pinnedToBottom = ref(true);
/** The transcript's content wrapper; the scrolling element is its ancestor. */
const transcriptContent = ref<HTMLElement | null>(null);
/** The `ScrollArea` viewport (`data-slot` of `ui/scroll-area/ScrollArea.vue`). */
const transcriptViewport = ref<HTMLElement | null>(null);
let transcriptResize: ResizeObserver | null = null;
/** The offset this panel itself last scrolled to (see `handleTranscriptScroll`). */
let programmaticOffset = 0;

/** Distance to the end under the slack still counts as "open at the bottom". */
function isAtBottom(element: HTMLElement): boolean {
  return element.scrollHeight - element.scrollTop - element.clientHeight <= BOTTOM_SLACK_PX;
}

/**
 * Re-reads the pin from the live scroll position. Only used where the movement
 * is the human's; programmatic scrolls go through `handleTranscriptScroll`.
 */
function syncPinnedToBottom(): void {
  const viewport = transcriptViewport.value;
  if (viewport === null) return;
  pinnedToBottom.value = isAtBottom(viewport);
}

/**
 * Moves the view to the latest message. Instant rather than smooth: a streaming
 * answer grows per token, and an animation would chase a moving target.
 */
function scrollToBottom(): void {
  const viewport = transcriptViewport.value;
  if (viewport === null) return;
  // Assigning past the maximum is clamped by the browser, so this also works
  // when the content is shorter than the viewport (nothing to scroll).
  viewport.scrollTop = viewport.scrollHeight;
  programmaticOffset = viewport.scrollTop;
  pinnedToBottom.value = true;
}

/**
 * The `scroll` listener of the viewport, so the pin follows every movement the
 * human makes (wheel, keyboard, scrollbar, touch) — not only the ones this panel
 * performs.
 *
 * A scroll event does not say who moved the view, and content can grow between
 * this panel's own `scrollTop` and the event it produces. An offset at or below
 * the one this panel set is therefore read as "still at the end, follow the new
 * bottom"; only an offset *above* it is the human moving up the conversation.
 */
function handleTranscriptScroll(): void {
  const viewport = transcriptViewport.value;
  if (viewport === null) return;
  if (pinnedToBottom.value && viewport.scrollTop >= programmaticOffset) {
    scrollToBottom();
    return;
  }
  pinnedToBottom.value = isAtBottom(viewport);
}

/**
 * A resize (window, drawer, composer) reflows the transcript: the flag is
 * re-read from the position, and a view that was glued stays glued.
 */
function handleViewportResize(): void {
  if (pinnedToBottom.value) {
    scrollToBottom();
    return;
  }
  syncPinnedToBottom();
}

function detachTranscriptViewport(): void {
  const viewport = transcriptViewport.value;
  if (viewport !== null) viewport.removeEventListener("scroll", handleTranscriptScroll);
  transcriptResize?.disconnect();
  transcriptResize = null;
  transcriptViewport.value = null;
  programmaticOffset = 0;
}

/**
 * The scrolling element of a `ScrollArea` is its viewport, not its root: the
 * wrapper only forwards attributes, so the listeners are bound to the viewport
 * found from the content the slot renders into.
 */
function attachTranscriptViewport(): void {
  const content = transcriptContent.value;
  const viewport =
    content?.closest<HTMLElement>('[data-slot="scroll-area-viewport"]') ?? null;
  transcriptViewport.value = viewport;
  if (viewport === null) return;
  viewport.addEventListener("scroll", handleTranscriptScroll, { passive: true });
  transcriptResize ??= new ResizeObserver(handleViewportResize);
  transcriptResize.observe(viewport);
  // A freshly mounted transcript has no position to preserve: open at the end.
  scrollToBottom();
}

watch(
  transcriptContent,
  () => {
    detachTranscriptViewport();
    attachTranscriptViewport();
  },
  // `post`: the slot content is patched by then, so the ancestor lookup and the
  // first scroll land on the real viewport.
  { flush: "post" },
);

// S5/S7: the model selector's data. `providerModels` is whatever Rust reports
// from pi's own file — grouped by provider, never a hardcoded list here — and
// `settings` holds the persisted choice (provider *and* model).
//
// The catalogue itself comes from `src/lib/provider-models.ts`, the module the
// settings pane writes to as well: hiding a model there, saving or discovering
// one, saving a provider or finishing a sign-in reloads that shared list, so this
// picker reflects the change without reopening anything. A failed read keeps the
// previous list (the store never blanks it out).
const settings = ref<AppSettings | null>(null);
const savingModel = ref(false);

/**
 * S2 of `models-by-provider.md`: how many models the grouped selector can offer.
 * A disabled model (L3) is not one of them, so a provider whose models are all
 * disabled leaves the selector disabled instead of empty.
 */
const enabledModelCount = computed(() =>
  providerModels.value.reduce(
    (total, entry) =>
      total + entry.models.filter((model) => model.enabled).length,
    0,
  ),
);

// S2/S4 of `theme-modes-and-resizable-drawer.md`: the appearance state lives in
// one module so the sidebar, the empty state and this panel all agree on the
// mode (the logo variant follows it); `src/lib/theme.ts` stays the only module
// that applies the palette.
const { mode: theme, isDark, setMode } = useAppearance();

/** Selected model id, or `""` until settings load. */
const modelId = computed(() => settings.value?.model ?? "");

/** S2: the provider the persisted model belongs to, `""` until settings load. */
const providerId = computed(() => settings.value?.provider ?? "");

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
      return { type: "info", text: t("chat.status.spawning") };
    case "ready":
      return { type: "success", text: t("chat.status.ready") };
    case "running":
      return { type: "info", text: t("chat.status.working") };
    case "stopped":
      return {
        type: "info",
        text:
          exit === null
            ? t("chat.status.stopped")
            : t("chat.status.stoppedExit", { code: exit }),
      };
    case "crashed":
      return {
        type: "error",
        text:
          runtimeError === null || runtimeError === ""
            ? t("chat.status.crashed")
            : t("chat.status.crashedError", { error: runtimeError }),
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
 * Everything that makes the transcript taller. Watching the signature keeps the
 * follow tied to real growth — streamed text, a closed turn, tool badges, the
 * status and error cards — instead of to one particular field.
 */
const transcriptSignature = computed(() =>
  [
    transcript.value.length,
    streamingText.value.length,
    tools.value.length,
    errorText.value?.length ?? 0,
    statusAlert.value?.text ?? "",
  ].join("|"),
);

/**
 * The follow itself: while the pin holds, every growth moves the view to the new
 * bottom; while it does not, nothing touches the scroll offset, so a human
 * reading higher up keeps the exact place they had.
 */
watch(
  transcriptSignature,
  () => {
    // `post` plus a tick: the scroll height read must be the grown one.
    void nextTick(() => {
      if (!pinnedToBottom.value) return;
      scrollToBottom();
    });
  },
  { flush: "post" },
);

/** The identity of the conversation on screen; `null` when there is none. */
const activeSessionId = computed(() => currentSession.value?.sessionId ?? null);

/**
 * S4: opening a session seeds the view at its latest message — once per switch,
 * plus whatever still arrives for it while the pin holds. Scrolling up drops the
 * pin, so the seed never fights the human afterwards.
 */
watch(activeSessionId, () => {
  pinnedToBottom.value = true;
  void nextTick(() => {
    scrollToBottom();
    // The history may still be loading; only keep following when we truly landed
    // at the end (a short conversation leaves nothing to scroll).
    syncPinnedToBottom();
  });
});

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
      return t("chat.tool.running");
    case "done":
      return t("chat.tool.done");
    case "error":
      return t("chat.tool.failed");
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
    panelError.value = t("chat.errors.noWorkspace");
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

/**
 * Reads the catalogue through the shared store and reports a failure on this
 * panel's error card. Nothing here owns the list: the store keeps it across
 * panels and across the settings pane's writes, and a failed read leaves the
 * previous answer in place instead of emptying the picker.
 */
async function refreshProviderModels(options: { force?: boolean } = {}) {
  const list = await loadProviderModels(options);
  // The store resolves instead of rejecting (so joining callers cannot diverge);
  // the reason travels separately and is surfaced where the human can read it.
  if (providerModelsError.value !== null) panelError.value = providerModelsError.value;
  return list;
}

/**
 * Coming back from the browser after a subscription sign-in is exactly when pi
 * may have refreshed its catalogue, and the window regaining focus is the moment
 * we learn about it. `force` because the store's answer is usually still fresh by
 * its own rule, and this read exists precisely to replace it.
 */
function handleWindowFocus(): void {
  void refreshProviderModels({ force: true });
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
 * S2 of `models-by-provider.md`: persists the chosen model together with the
 * provider it belongs to. Model ids are provider-scoped, so writing only the id
 * could point the next session at another provider's model (or at nothing).
 *
 * The whole settings file is re-read first so the write cannot clobber a
 * preference the drawer changed in the meantime (`save_settings` replaces the
 * file). The choice applies to the next spawn.
 */
async function handleSelectModel(choice: { provider: string; model: string }) {
  if (choice.model === "" || choice.provider === "") return;
  savingModel.value = true;
  panelError.value = null;
  try {
    const current = await getSettings();
    settings.value = await saveSettings({
      ...current,
      provider: choice.provider,
      model: choice.model,
    });
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
    // The drawer sorts by activity and caches each workspace's list, so every
    // successful send has to invalidate it — not just the first turn, which was
    // only there to pick up the derived alias.
    sessionGeneration.value += 1;
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
  window.addEventListener("focus", handleWindowFocus);
  // Presentation-only additions (S5): the header needs the active space's name
  // (already a prop) plus the provider-grouped models and the persisted choice.
  // The models are read through the shared store, so a write of the settings pane
  // is already reflected here.
  void refreshProviderModels();
  void loadSettings();
  // L3/S4: the store owns the single `agent-event` subscription and buffers
  // every session, so this panel only asks for it to be installed. It is never
  // torn down with the panel: a background session must keep receiving events.
  void ensureSessionEvents();
});

onUnmounted(() => {
  // Only the focus listener and the transcript listeners are ours: the
  // agent-event subscription belongs to the store and must survive a panel swap.
  window.removeEventListener("focus", handleWindowFocus);
  detachTranscriptViewport();
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
        :aria-label="t('chat.expandTools')"
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
      :hint="workspace === null ? t('chat.noWorkspaceHint') : null"
    >
      <template #composer>
        <Composer
          v-model="prompt"
          :provider-id="providerId"
          :model-id="modelId"
          :provider-models="providerModels"
          :workspace-name="workspace?.name ?? t('chat.noWorkspace')"
          :streaming="sending"
          :disabled="workspace === null"
          :can-send="canSend"
          :model-disabled="savingModel || enabledModelCount === 0"
          :can-stop="currentSession !== null && !stopping"
          @select-model="handleSelectModel"
          @send="handleSend"
          @stop="handleStop"
        />
      </template>
    </ChatEmptyState>

    <!-- S2: conversation state — header, Chat/Linaje tabs, transcript, composer. -->
    <template v-else>
      <ChatHeader
        :title="workspaceName"
        :mode="t('chat.standardMode')"
        :tools-open="toolsOpen"
        :is-dark="isDark"
        @toggle-tools="emit('toggleTools')"
        @toggle-theme="handleToggleTheme"
      />

      <Tabs default-value="chat" class="min-h-0 flex-1 gap-0">
        <div class="border-b border-border px-4">
          <TabsList variant="line" class="h-9">
            <TabsTrigger value="chat">{{ t("chat.tab.chat") }}</TabsTrigger>
            <TabsTrigger value="linaje">{{ t("chat.tab.lineage") }}</TabsTrigger>
          </TabsList>
        </div>

        <TabsContent value="chat" class="flex min-h-0 flex-1 flex-col">
          <!-- Transcript: the only scrolling region of the panel. -->
          <ScrollArea class="min-h-0 flex-1">
            <div ref="transcriptContent" class="flex flex-col gap-4 p-4">
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
                  <span class="text-xs font-semibold">{{ t("chat.error.label") }}</span>
                </div>
                <p class="mt-1 text-sm text-muted-foreground">{{ errorText }}</p>
                <div class="mt-2 flex items-center justify-end">
                  <span
                    class="text-[10px] font-medium tracking-wider text-muted-foreground uppercase"
                  >
                    {{ t("chat.error.tag") }}
                  </span>
                </div>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  class="absolute top-1.5 right-1.5"
                  :aria-label="t('common.dismiss')"
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
                {{ t("chat.loadingConversation") }}
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
              :provider-id="providerId"
              :model-id="modelId"
              :provider-models="providerModels"
              :workspace-name="workspace?.name ?? t('chat.noWorkspace')"
              :streaming="sending"
              :disabled="workspace === null"
              :can-send="canSend"
              :model-disabled="savingModel || enabledModelCount === 0"
              :can-stop="currentSession !== null && !stopping"
              @select-model="handleSelectModel"
              @send="handleSend"
              @stop="handleStop"
            />
            <p class="mt-2 text-center text-xs text-muted-foreground">
              {{ t("chat.turnsAndSteps", { turns, steps }) }}
            </p>
          </div>
        </TabsContent>

        <TabsContent
          value="linaje"
          class="flex min-h-0 flex-1 items-center justify-center"
        >
          <p class="text-center text-sm text-muted-foreground">
            {{ t("chat.lineagePlaceholder") }}
          </p>
        </TabsContent>
      </Tabs>
    </template>
  </div>
</template>
