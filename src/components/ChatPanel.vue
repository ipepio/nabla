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
import QuestionCard from "@/components/chat/QuestionCard.vue";
import { draftKeyFor, usePromptDraft } from "@/components/chat/use-prompt-draft";
import { useThrottledText } from "@/components/chat/use-throttled-text";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  answerUiRequest,
  commandErrorMessage,
  getSettings,
  saveSettings,
  sessionStatus,
  stopSession,
} from "@/contracts/commands";
import type {
  AppSettings,
  SessionDto,
  TranscriptRole,
  UiAnswer,
  WorkspaceInfo,
} from "@/contracts/types";
import { useAppearance } from "@/lib/appearance";
import { t } from "@/lib/i18n";
import {
  COMPOSER_MAX_WIDTH,
  CONVERSATION_COMPOSER_MAX_WIDTH,
  CONVERSATION_TRANSCRIPT_MAX_WIDTH,
} from "@/lib/layout";
import {
  loadProviderModels,
  providerModels,
  providerModelsError,
} from "@/lib/provider-models";
import {
  activeSession,
  ensureSessionEvents,
  enqueuePrompt,
  isSessionSpawning,
  isTranscriptLoading,
  newSession,
  pendingQuestions,
  queuedCount,
  queuedPrompts,
  recordSessionStatus,
  removeQueuedPrompt,
  resolveQuestion,
  sendUserTurn,
  sessionBuffer,
  type PendingQuestion,
  type SessionBuffer,
  type ToolEntry,
} from "@/lib/session-store";
import { applyPreferences, queueMessages, showReasoning } from "@/lib/preferences";
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

/**
 * The click has been taken but pi has not accepted it yet: session creation plus
 * the first RPC round-trip. It is a synchronous latch (set before the first
 * `await`, so a second Enter cannot start a second session) and nothing more —
 * the moment a send is accepted, the store's `busy` takes over. Both are read
 * through the single `agentWorking` below, which is what the composer's stop,
 * send/queue decision and disabled state all derive from.
 */
const sending = ref(false);
/** Same latch for the "there is no session yet" half of the first send. */
const starting = ref(false);
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
 * P2 of `chat-polish.md`: where this panel's unsent text is persisted. A session
 * has its own draft; the state before the first send has no session yet (it is
 * created when the prompt is handed to pi), so it is keyed by workspace instead —
 * that is what brings back a prompt typed in a workspace and left unsent.
 */
const draftKey = computed<string | null>(() =>
  draftKeyFor(
    currentSession.value?.sessionId ?? null,
    props.workspace?.id ?? null,
  ),
);

/**
 * S2/P2: the composer's text *is* the draft of what it addresses. The composable
 * restores on a key change (session or workspace switch), saves ~300 ms after the
 * last keystroke and forgets the draft on `clear()`, which is what sending and
 * queueing call. Nothing here surfaces a storage failure: the feature degrades to
 * "not persisted" and the composer keeps working.
 */
const draft = usePromptDraft(() => draftKey.value);
const prompt = draft.text;

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
/**
 * The reasoning still streaming for the turn in progress. Rendered by the same
 * `ReasoningBlock` as a completed turn's, under the id the turn will flush into,
 * so opening it mid-stream keeps it open afterwards.
 */
const streamingReasoning = computed(() => buffer.value?.streamingReasoning ?? "");
const liveMessageId = computed(() => buffer.value?.liveMessageId);

/**
 * P1 of `chat-polish.md`: the live tail is parsed at most every ~120 ms instead
 * of once per delta. Everything that renders the *streaming* bubble reads this
 * value — never the raw buffer field — so what is painted and what is throttled
 * can never disagree.
 *
 * The commit at turn end needs no special case here: `flushStreaming` clears
 * `streamingText` in the same tick it appends the committed message, and the
 * composable publishes a tail that stopped extending what it had painted
 * immediately (see `use-throttled-text.ts`). The committed message is rendered
 * from its own text, so the final words on screen never wait for this throttle.
 */
const liveText = useThrottledText(
  streamingText,
  () => liveMessageId.value ?? null,
);
const transcriptLoading = computed(() =>
  isTranscriptLoading(currentSession.value?.sessionId ?? null),
);

/**
 * The muted "loading conversation" note, and the only condition under which it
 * belongs on screen: there is genuinely nothing to show yet.
 *
 * A session that painted its preview (or pi's file) already has a conversation, and
 * one whose turn is already streaming has live text — a loading note above either
 * of them would be a second status that contradicts what the human is reading.
 * Those sessions wait silently instead, which is what makes opening one feel
 * instant: the messages appear first and the engine finishes booting behind them.
 */
const showsLoadingPlaceholder = computed(
  () =>
    transcriptLoading.value &&
    transcript.value.length === 0 &&
    streamingText.value === "" &&
    streamingReasoning.value === "",
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
  // The content element is observed too: collapsing the reasoning block (or a
  // code block growing) changes its height without touching the viewport, and a
  // view glued to the bottom must follow that change as well.
  if (content !== null) transcriptResize.observe(content);
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
    current.streamingReasoning === "" &&
    current.tools.length === 0 &&
    errorText.value === null
  );
});

/**
 * Is the agent doing work right now?
 *
 * The store's `busy` is the authority: it goes up the instant a prompt is handed
 * to pi (direct send *and* queue drain) and comes down on `turn_end`,
 * `agent_end`, `process_exit`, a stop or a crash, so it also covers the silence
 * between acceptance and pi's first delta. `sending`/`starting` only extend it
 * over the milliseconds before a send exists to accept.
 *
 * The composer receives exactly this as `streaming`, and Stop, the send/queue
 * decision and `canSend` are all derived from it — the same field the drawer's
 * spinner reads through `busyIds`, so the two surfaces cannot disagree.
 */
const agentWorking = computed(
  () => sending.value || starting.value || (buffer.value?.busy ?? false),
);

/**
 * The prompt can be sent right now: there is a workspace, something to say, and
 * the agent is not working. "Not working" reads `agentWorking`, not a private
 * copy of it, so a submit and the stop control can never disagree about whether
 * the turn is over.
 */
const canSend = computed(
  () => props.workspace !== null && prompt.value.trim() !== "" && !agentWorking.value,
);

/**
 * S3: the prompt can be queued instead of sent — the agent is working, the
 * preference is on, there is a session to queue it against and there is
 * something to queue. The store keeps the queue per session; the panel only
 * decides which event the composer emits.
 *
 * `sending`/`starting` keep it off while a click is being accepted: the text is
 * still in the composer then, and it belongs to that send — queueing it as well
 * would show the same prompt twice.
 */
const canQueue = computed(
  () =>
    queueMessages.value &&
    agentWorking.value &&
    !sending.value &&
    !starting.value &&
    currentSession.value !== null &&
    props.workspace !== null &&
    prompt.value.trim() !== "",
);

/** Prompts of the session on screen that are waiting for a turn to end (S3). */
const queuedForSession = computed(() =>
  queuedCount(currentSession.value?.sessionId ?? null),
);

/**
 * The queued prompts themselves, for the list above the composer. The store
 * hands back a copy of the reactive queue, so adding, sending or removing one
 * re-runs this computed and the panel never owns the list.
 */
const queuedList = computed(() =>
  queuedPrompts(currentSession.value?.sessionId ?? null),
);

/**
 * S3 of `agent-question-bridge.md`: the questions this session's agent is blocked
 * on, in arrival order (pi allows concurrent dialogs, so it is a stack). The
 * store owns the list — it clears it when the process stops — so the panel only
 * renders it and answers it.
 */
const questions = computed(() =>
  pendingQuestions(currentSession.value?.sessionId ?? null),
);

/**
 * Id of the question whose answer is on its way. While set, every card is inert:
 * a second click would write a second answer for a question pi has already
 * resolved, and the protocol has no way to take an answer back.
 */
const answering = ref<string | null>(null);

/**
 * The queue stack shares the composer's ceiling, so it lines up with the box
 * under it: the empty state's narrow one, the conversation's wide one.
 */
const queueMaxWidth = computed(
  () =>
    `${isEmptyState.value ? COMPOSER_MAX_WIDTH : CONVERSATION_COMPOSER_MAX_WIDTH}px`,
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
    // The painted tail, not the buffer's: the follow tracks what grew on screen,
    // so it steps with the throttle instead of once per delta.
    liveText.value.length,
    streamingReasoning.value.length,
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
    // S2/S3 of `chat-preferences-and-queue.md`: the chat preferences live in the
    // shared module, so the settings dialog and this panel read the same values.
    applyPreferences(settings.value);
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

/**
 * Send and Queue share this guard.
 *
 * A send goes through `sendUserTurn`, which writes the human's turn to the
 * transcript and marks the session working before the bridge is asked, and rolls
 * both back if the send does not land. That is why nothing here appends a turn
 * itself: the store owns the conversation, and a failed send must not leave a
 * question pi never received in it.
 *
 * The one exception is a session whose pi is still starting. Creating one now
 * answers `spawning` at once, but pi's stdin does not exist until its process is
 * ready, so sending would only be refused. The prompt takes the queue path
 * instead — the same queue a working agent uses, never a second one — and the
 * store sends it the moment the process reports live. That is what makes the
 * composer usable in the same instant the session appears.
 */
async function handleSend() {
  const text = prompt.value.trim();
  if (text === "") return;
  // The key the text was typed under, before a send can create its session.
  const originKey = draftKey.value;
  // S3: with the queue preference on, a prompt typed while the agent works is
  // queued instead of dropped. The guard used to return here for both cases,
  // which silently discarded what the human had just typed — the toggle said
  // "Encolar mensajes" and this path ignored it. The queue is the same one the
  // explicit action fills, so a queued prompt looks and drains the same way.
  if (agentWorking.value) {
    if (queueMessages.value) handleQueue();
    return;
  }
  sending.value = true;
  panelError.value = null;
  try {
    const current = await ensureSession();
    if (current === null) return;
    if (isSessionSpawning(current.sessionId)) {
      enqueuePrompt(current.sessionId, text, current.workspaceId);
      draft.clear(originKey);
      return;
    }
    await sendUserTurn(current.sessionId, current.workspaceId, text);
    draft.clear(originKey);
  } catch (cause) {
    // The composer keeps the text: a prompt that never left is still the
    // human's to resend.
    panelError.value = commandErrorMessage(cause);
  } finally {
    sending.value = false;
  }
}

/**
 * S3 of `chat-preferences-and-queue.md`: the human submitted while the agent is
 * working and the queue is on. The prompt is kept in this session's queue and
 * pi is not called at all — `session-store` sends it once the current turn ends.
 *
 * The store also puts the turn in the transcript right now, muted and marked
 * pending, so the question the human typed is visible and in order while it
 * waits; sending it later only clears that mark. The composer is cleared so the
 * next prompt starts empty, exactly as a send does.
 */
function handleQueue() {
  const current = currentSession.value;
  const text = prompt.value.trim();
  // The latch check matters: while a send is being accepted the text belongs to
  // that send, and queueing it too would show one prompt twice.
  if (sending.value || starting.value) return;
  if (current === null || text === "" || !queueMessages.value) return;
  // The panel knows the workspace of the session it renders, so the queued
  // prompt is addressable the moment the turn ends — even if the human has
  // moved to another workspace by then.
  enqueuePrompt(current.sessionId, text, current.workspaceId);
  // P2: a queued prompt is not unsent either, so the session's draft goes too.
  draft.clear();
}

/**
 * S3: drops one waiting prompt of the session on screen. Index-based because the
 * list is rendered in queue order; the store ignores an index that has since
 * drained, so a click racing a turn end cannot throw.
 */
function handleRemoveQueued(index: number) {
  removeQueuedPrompt(currentSession.value?.sessionId ?? null, index);
}

/**
 * S3: answers one pending question, which is what unblocks the agent's turn.
 *
 * The card leaves only once the line really reached the child
 * (`answer_ui_request` rejects for a session that is not running), so a card
 * that is gone always means the agent has the answer. A failure keeps it on
 * screen with the reason in the error card, which is the honest state: still
 * unanswered.
 */
async function handleAnswer(question: PendingQuestion, answer: UiAnswer) {
  const current = currentSession.value;
  if (current === null || answering.value !== null) return;
  answering.value = question.id;
  panelError.value = null;
  try {
    await answerUiRequest(
      current.workspaceId,
      Number(current.sessionId),
      question.id,
      answer,
    );
    resolveQuestion(current.sessionId, question.id);
  } catch (cause) {
    panelError.value = commandErrorMessage(cause);
  } finally {
    answering.value = null;
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
        <div class="flex w-full flex-col gap-1.5">
          <!-- S3 of `agent-question-bridge.md`: what the agent is blocked on,
               answered here or cancelled. Above the queue: an unanswered
               question is the turn, a queued prompt is only what comes next. -->
          <div
            v-if="questions.length > 0"
            class="mx-auto flex w-full flex-col gap-1.5"
            :style="{ maxWidth: queueMaxWidth }"
          >
            <QuestionCard
              v-for="question in questions"
              :key="question.id"
              :question="question"
              :busy="answering !== null"
              @answer="handleAnswer(question, $event)"
            />
          </div>

          <!-- S3: this session's waiting prompts, directly above the composer
               (see the conversation branch below for the same stack). -->
          <div
            v-if="queuedList.length > 0"
            class="mx-auto flex w-full flex-col gap-1"
            :style="{ maxWidth: queueMaxWidth }"
          >
            <p class="px-1 text-xs text-muted-foreground">
              {{ t("chat.queue.heading", { count: queuedList.length }) }}
            </p>
            <ul
              class="max-h-[5rem] overflow-y-auto rounded-md bg-muted/40 p-1"
              :aria-label="t('chat.queue.list')"
            >
              <li
                v-for="(item, index) in queuedList"
                :key="`${index}-${item}`"
                class="flex items-center gap-1.5"
              >
                <span
                  class="w-4 shrink-0 text-right text-[10px] tabular-nums text-muted-foreground"
                  aria-hidden="true"
                >
                  {{ index + 1 }}
                </span>
                <span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                  {{ item }}
                </span>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  class="shrink-0"
                  :aria-label="t('chat.queue.remove', { index: index + 1 })"
                  @click="handleRemoveQueued(index)"
                >
                  <XIcon />
                </Button>
              </li>
            </ul>
          </div>

          <Composer
            v-model="prompt"
            :max-width="COMPOSER_MAX_WIDTH"
            :provider-id="providerId"
            :model-id="modelId"
            :provider-models="providerModels"
            :workspace-name="workspace?.name ?? t('chat.noWorkspace')"
            :streaming="agentWorking"
            :disabled="workspace === null"
            :can-send="canSend"
            :can-queue="canQueue"
            :queued-count="queuedForSession"
            :model-disabled="savingModel || enabledModelCount === 0"
            :can-stop="currentSession !== null && !stopping"
            @select-model="handleSelectModel"
            @send="handleSend"
            @queue="handleQueue"
            @stop="handleStop"
          />
        </div>
      </template>
    </ChatEmptyState>

    <!-- S2: conversation state — header, transcript, composer. -->
    <template v-else>
      <ChatHeader
        :title="workspaceName"
        :mode="t('chat.standardMode')"
        :tools-open="toolsOpen"
        :is-dark="isDark"
        @toggle-tools="emit('toggleTools')"
        @toggle-theme="handleToggleTheme"
      />

      <!-- No tab strip any more: the panel is one pane, so the `v-else` children
           are the flex items themselves and the ScrollArea keeps filling the
           height between the header and the composer. -->
      <!-- Transcript: the only scrolling region of the panel. The column is
           centred and bounded (a maximum, not a floor): the header and the
           composer still span the panel, only the conversation is held to a
           readable measure. -->
      <ScrollArea class="min-h-0 flex-1">
        <div
          ref="transcriptContent"
          class="mx-auto flex w-full flex-col gap-4 p-4"
          :style="{ maxWidth: `${CONVERSATION_TRANSCRIPT_MAX_WIDTH}px` }"
        >
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

          <!-- S3: nothing to show yet, so the only honest status is that the
               conversation is being read. A session that already painted its
               preview never gets here. -->
          <p
            v-if="showsLoadingPlaceholder"
            class="text-sm text-muted-foreground"
          >
            {{ t("chat.loadingConversation") }}
          </p>

          <!-- L3/S4: the active session's buffer, which kept filling while
               this session was off screen. -->
          <template v-for="message in transcript" :key="message.id">
            <div
              v-if="message.role === 'tool'"
              class="flex items-start gap-2 text-xs text-muted-foreground"
            >
              <WrenchIcon class="mt-0.5 size-3 shrink-0" aria-hidden="true" />
              <span class="min-w-0 whitespace-pre-wrap">{{ message.text }}</span>
            </div>
            <!-- A queued prompt is not here yet: it lives in the queue strip
                 above the composer until it is handed to pi, and only then
                 becomes a turn in the conversation. -->
            <MessageBubble
              v-else
              :role="bubbleRole(message.role)"
              :text="message.text"
              :at="timeLabel(message.at)"
              :reasoning="showReasoning ? message.reasoning : undefined"
              :message-id="message.id"
            />
          </template>

          <!-- The turn still streaming: it becomes a transcript entry when
               pi closes the message. Reasoning streams above the answer and
               keeps its own open/closed state under the id it flushes into.
               S2: with "Show reasoning" off the block is not rendered at all —
               not merely collapsed — for this turn and for every stored one. -->
          <MessageBubble
            v-if="
              liveText !== '' ||
              (showReasoning && streamingReasoning !== '')
            "
            role="assistant"
            :text="liveText"
            :reasoning="
              showReasoning && streamingReasoning !== ''
                ? streamingReasoning
                : undefined
            "
            :message-id="liveMessageId ?? undefined"
            reasoning-live
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

      <!-- Composer, pinned under the transcript, plus the turns/steps meta.
           No rule above it: the conversation and its input read as one
           surface instead of two panes. -->
      <div class="px-4 py-3">
        <!-- S3 of `agent-question-bridge.md`: the same card stack as the empty
             state above, so a question is answered in the same place whichever
             state the panel is in. -->
        <div
          v-if="questions.length > 0"
          class="mx-auto mb-1.5 flex w-full flex-col gap-1.5"
          :style="{ maxWidth: queueMaxWidth }"
        >
          <QuestionCard
            v-for="question in questions"
            :key="question.id"
            :question="question"
            :busy="answering !== null"
            @answer="handleAnswer(question, $event)"
          />
        </div>

        <!-- S3: this session's waiting prompts, directly above the composer and
             capped to three visible rows so the input never leaves the screen.
             Same stack as the empty state above. -->
        <div
          v-if="queuedList.length > 0"
          class="mx-auto mb-1.5 flex w-full flex-col gap-1"
          :style="{ maxWidth: queueMaxWidth }"
        >
          <p class="px-1 text-xs text-muted-foreground">
            {{ t("chat.queue.heading", { count: queuedList.length }) }}
          </p>
          <ul
            class="max-h-[5rem] overflow-y-auto rounded-md bg-muted/40 p-1"
            :aria-label="t('chat.queue.list')"
          >
            <li
              v-for="(item, index) in queuedList"
              :key="`${index}-${item}`"
              class="flex items-center gap-1.5"
            >
              <span
                class="w-4 shrink-0 text-right text-[10px] tabular-nums text-muted-foreground"
                aria-hidden="true"
              >
                {{ index + 1 }}
              </span>
              <span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                {{ item }}
              </span>
              <Button
                variant="ghost"
                size="icon-xs"
                class="shrink-0"
                :aria-label="t('chat.queue.remove', { index: index + 1 })"
                @click="handleRemoveQueued(index)"
              >
                <XIcon />
              </Button>
            </li>
          </ul>
        </div>

        <Composer
          v-model="prompt"
          :max-width="CONVERSATION_COMPOSER_MAX_WIDTH"
          :provider-id="providerId"
          :model-id="modelId"
          :provider-models="providerModels"
          :workspace-name="workspace?.name ?? t('chat.noWorkspace')"
          :streaming="agentWorking"
          :disabled="workspace === null"
          :can-send="canSend"
          :can-queue="canQueue"
          :queued-count="queuedForSession"
          :model-disabled="savingModel || enabledModelCount === 0"
          :can-stop="currentSession !== null && !stopping"
          @select-model="handleSelectModel"
          @send="handleSend"
          @queue="handleQueue"
          @stop="handleStop"
        />
        <p class="mt-2 text-center text-xs text-muted-foreground">
          {{ t("chat.turnsAndSteps", { turns, steps }) }}
        </p>
      </div>
    </template>
  </div>
</template>
