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
import { computed, defineAsyncComponent, nextTick, onMounted, onUnmounted, reactive, ref, watch } from "vue";
import {
  LoaderCircleIcon,
  PanelRightOpenIcon,
  XIcon,
} from "@lucide/vue";
import ChatEmptyState from "@/components/chat/ChatEmptyState.vue";
import ThemeToggle from "@/components/chat/ThemeToggle.vue";
import ChatHeader from "@/components/chat/ChatHeader.vue";
import Composer from "@/components/chat/Composer.vue";
import QuestionCard from "@/components/chat/QuestionCard.vue";
import QuestionPrompt from "@/components/chat/QuestionPrompt.vue";
const SubagentView = defineAsyncComponent(
  () => import("@/components/chat/SubagentView.vue"),
);
import TurnGroup from "@/components/chat/TurnGroup.vue";
import OlderTurnsControl from "@/components/chat/OlderTurnsControl.vue";
import { deriveTurnGroups } from "@/components/chat/turn-groups";
import {
  COMPOSER_IMAGE_LIMIT,
  COMPOSER_IMAGE_MAX_BYTES,
  parseImageDataUrl,
  type ImageCandidate,
  type ImageRejection,
} from "@/components/chat/composer-images";
import {
  COMPOSER_PDF_LIMIT,
  COMPOSER_PDF_MAX_PAGES,
  hasUnreadablePdf,
  type ComposerPdf,
  type PdfRejection,
} from "@/components/chat/composer-pdfs";
import type { ComposerToolIntent } from "@/components/chat/composer-tools";
import { composeTurnDisplay, composeTurnPrompt } from "@/components/chat/composer-turn";
import { draftKeyFor, usePromptDraft } from "@/components/chat/use-prompt-draft";
import { useThrottledText } from "@/components/chat/use-throttled-text";
import { createTranscriptFollow } from "@/components/chat/transcript-follow";
import { useTurnWindow } from "@/components/chat/turn-window";
import { formatTranscriptMarkdown } from "@/components/chat/export-markdown";
import {
  contextUsageRatio,
  estimateTokens,
  formatCompactTokens,
  formatElapsed,
} from "@/components/chat/session-metrics";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  answerUiRequest,
  commandErrorMessage,
  debugLog,
  getSettings,
  mcpEndpoint,
  pickImageFiles,
  pickPdfFiles,
  readImageAttachment,
  readPdfAttachment,
  saveSettings,
  sessionStatus,
  setSessionModel,
  setSessionThinkingLevel,
  stopSession,
} from "@/contracts/commands";
import type {
  AppSettings,
  SessionDto,
  UiAnswer,
  WorkspaceInfo,
} from "@/contracts/types";
import { useAppearance } from "@/lib/appearance";
import { takeBootSettings } from "@/lib/boot-settings";
import { sendAppNotification } from "@/lib/notifications";
import {
  addAttachments,
  addPdfAttachment,
  attachmentsFor,
  attachmentsToPromptImages,
  clearAttachments,
  intentsFor,
  moveAttachments,
  pdfsFor,
  removeAttachment,
  removePdfAttachment,
  toggleIntent,
} from "@/lib/composer-attachments";
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
  activeSessionTitle,
  ensureSessionEvents,
  enqueuePrompt,
  isSessionSpawning,
  isTranscriptLoading,
  liveIds,
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
import {
  applyPreferences,
  isToolEnabled,
  queueMessages,
  showReasoning,
  webSearchEnabled,
} from "@/lib/preferences";
import {
  closeSubagentView,
  isSubagentActive,
  openSubagent,
  subagentChildrenOf,
} from "@/lib/subagent-store";
import { isDarkMode, type ThemeMode } from "@/lib/theme";

/** The drawer owns the workspace list; the active one arrives already chosen. */
const props = defineProps<{
  workspace: WorkspaceInfo | null;
  /** Right tools drawer visibility (App.vue owns the state). */
  toolsOpen: boolean;
}>();

const emit = defineEmits<{
  (e: "toggleTools"): void;
  (e: "openSearch"): void;
}>();

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
/**
 * The prompt an accepted send is waiting on, or `null`.
 *
 * `sending` covers the whole round-trip (session creation plus the first RPC),
 * and the composer still holds that send's text until pi accepts it — so "a send
 * is in flight" alone cannot say whether the text on screen belongs to it or is a
 * new prompt the human typed after clicking. Comparing the text is what tells the
 * two apart, and it is the only thing that may keep a new prompt out of the queue
 * while a send is in flight.
 */
const inFlightPrompt = ref<string | null>(null);
/**
 * How many images that in-flight send carries. The text alone cannot tell a new
 * prompt from the one already on its way when the turn is image-only (both have
 * empty text), so ownership is the pair.
 */
const inFlightImages = ref(0);
/**
 * S3 of `composer-tools-pdf.md`: how many PDFs and tool requests that send
 * carries. Ownership is everything the turn is made of, so a prompt typed after
 * the click — even with the same words — is never mistaken for the one on its
 * way, and the document of one draft cannot be confused with another's.
 */
const inFlightPdfs = ref(0);
const inFlightIntents = ref(0);
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
 * S1-S2 of `odd/tasks/composer-image-input.md`: the images attached to the draft on
 * screen. They belong to the same key the text is persisted under, so switching
 * conversations switches them too (`src/lib/composer-attachments.ts`), and they
 * live in memory only — no base64 ever reaches localStorage, a log line or SQLite.
 */
const images = computed(() => [...attachmentsFor(draftKey.value)]);

/**
 * S3 of `odd/tasks/composer-tools-pdf.md`: the PDFs attached to the draft on
 * screen. Same key and same memory-only lifetime as the images, so switching
 * conversations switches them too and nothing extracted ever reaches storage.
 */
const pdfs = computed<ComposerPdf[]>(() => [...pdfsFor(draftKey.value)]);

/**
 * S2: the tool requests chosen for this draft, oldest first. They are text the
 * next turn carries, never a setting and never an execution.
 */
const intents = computed<ComposerToolIntent[]>(() => [...intentsFor(draftKey.value)]);

/**
 * S3: a PDF with no selectable text (a scan) cannot be sent — Nabla runs no OCR,
 * so its pages carry nothing and the composer says so instead of pretending.
 */
const pdfUnreadable = computed(() => hasUnreadablePdf(pdfs.value));

/** How many PDFs are being extracted right now, so the composer can say so. */
const pdfReading = ref(0);

/**
 * S2 of `composer-tools-pdf.md`: whether this run bound the MCP endpoint the
 * agent's tools are published from (`web_search` and the browser's). Read once,
 * because it is a property of the process: no endpoint means the agent has
 * neither, and the menu says that instead of offering a request nothing can
 * answer — whatever `settings.json` holds.
 */
const toolEndpointReady = ref(false);

/**
 * S2 of `composer-tools-pdf.md`: the Browser request needs the Browser tool the
 * human turned on in Settings → Tools, read from the shared preference ref so a
 * flip in the dialog reaches the composer's `+` menu without a reload. The search
 * preference is the other switch; neither is inferred from the endpoint being up.
 */
const browserToolEnabled = computed(() => isToolEnabled("browser"));

/**
 * Why an image was not attached, as the human reads it. `composer-images.ts`
 * owns the bounds; this only turns a rejection into words.
 */
function rejectionMessage(rejection: ImageRejection): string {
  switch (rejection) {
    case "too-many":
      return t("chat.composer.imageTooMany", { count: COMPOSER_IMAGE_LIMIT });
    case "too-large":
      return t("chat.composer.imageTooLarge", {
        limit: Math.round(COMPOSER_IMAGE_MAX_BYTES / (1024 * 1024)),
      });
    default:
      return t("chat.composer.imageUnsupportedType");
  }
}

/** Adds accepted candidates to the draft and reports the first rejection. */
function attachCandidates(candidates: ImageCandidate[]): void {
  const key = draftKey.value;
  if (key === null || candidates.length === 0) return;
  const { added, rejection } = addAttachments(key, candidates);
  debugLog(
    `composer images: +${added}${rejection === null ? "" : ` (${rejection}, ${candidates.length} offered)`}`,
  );
  if (rejection !== null) panelError.value = rejectionMessage(rejection);
}

/**
 * Reads the files Rust was asked for and attaches them. Rust sniffs the format from
 * the bytes and enforces the size ceiling, so one unreadable or oversized file
 * reports its own reason without discarding the others.
 */
async function addImagePaths(paths: string[]): Promise<void> {
  if (paths.length === 0) return;
  const candidates: ImageCandidate[] = [];
  for (const path of paths) {
    try {
      const file = await readImageAttachment(path);
      candidates.push({
        name: file.name,
        mimeType: file.mimeType,
        data: file.data,
      });
    } catch (cause) {
      panelError.value = commandErrorMessage(cause);
    }
  }
  attachCandidates(candidates);
}

/** S1: the `+` button — the system picker, filtered to pi's inline formats. */
async function handlePickImages(): Promise<void> {
  panelError.value = null;
  try {
    await addImagePaths(await pickImageFiles());
  } catch (cause) {
    panelError.value = commandErrorMessage(cause);
  }
}

/** Why a PDF was not attached, as the human reads it. */
function pdfRejectionMessage(rejection: PdfRejection): string {
  switch (rejection) {
    case "too-many":
      return t("chat.composer.pdfTooMany", { count: COMPOSER_PDF_LIMIT });
    case "too-many-pages":
      return t("chat.composer.pdfTooManyPages", { count: COMPOSER_PDF_MAX_PAGES });
    case "too-large":
      return t("chat.composer.pdfTooLarge");
    default:
      return t("chat.composer.pdfEmpty");
  }
}

/**
 * S3: extracts the PDFs Rust was asked for, one at a time, so one unreadable
 * document reports its own reason without discarding the others.
 *
 * Rust owns the extraction (local `pdftotext`, argv-only, bounded in time, size,
 * pages and characters); this only turns its answer — or its refusal — into an
 * attached document the composer can show and remove.
 */
async function addPdfPaths(paths: string[]): Promise<void> {
  if (paths.length === 0) return;
  for (const path of paths) {
    pdfReading.value += 1;
    try {
      const extracted = await readPdfAttachment(path);
      const { added, rejection } = addPdfAttachment(draftKey.value, extracted);
      debugLog(
        `composer pdf: +${added} pages=${extracted.pageCount} hasText=${extracted.hasText}${rejection === null ? "" : ` (${rejection})`}`,
      );
      if (rejection !== null) panelError.value = pdfRejectionMessage(rejection);
    } catch (cause) {
      panelError.value = commandErrorMessage(cause);
    } finally {
      pdfReading.value -= 1;
    }
  }
}

/** S3: the menu's PDF entry — the system picker, filtered to PDF files. */
async function handlePickPdf(): Promise<void> {
  panelError.value = null;
  try {
    await addPdfPaths(await pickPdfFiles());
  } catch (cause) {
    panelError.value = commandErrorMessage(cause);
  }
}

/** S3: the PDF chip's own undo — the document simply was not sent. */
function handleRemovePdf(id: string): void {
  removePdfAttachment(draftKey.value, id);
}

/** S2: adds or removes one tool request of this draft; nothing else happens. */
function handleToggleIntent(intent: ComposerToolIntent): void {
  toggleIntent(draftKey.value, intent);
  debugLog(`composer tool request: ${intent} ${intents.value.includes(intent) ? "on" : "off"}`);
}

/** Reads one pasted `Blob` as the data url `parseImageDataUrl` expects. */
function readFileAsDataUrl(file: Blob): Promise<string | null> {
  return new Promise((resolve) => {
    const reader = new FileReader();
    reader.onerror = () => resolve(null);
    reader.onload = () =>
      resolve(typeof reader.result === "string" ? reader.result : null);
    reader.readAsDataURL(file);
  });
}

/** S1: a screenshot pasted into the composer (text pastes keep their path). */
async function handlePasteImages(files: File[]): Promise<void> {
  const candidates: ImageCandidate[] = [];
  for (const file of files) {
    const url = await readFileAsDataUrl(file);
    const image = url === null ? null : parseImageDataUrl(url);
    if (image === null) {
      panelError.value = t("chat.composer.imageUnsupportedType");
      continue;
    }
    candidates.push({ name: file.name, mimeType: image.mimeType, data: image.data });
  }
  attachCandidates(candidates);
}

/** S1: files dropped on the composer, read through the same path as the picker. */
function handleDropFiles(paths: string[]): void {
  // S3: a dropped PDF is extracted, a dropped image is read as an image; the
  // extension only routes the path, the bytes are still checked by Rust.
  const pdfPaths: string[] = [];
  const imagePaths: string[] = [];
  for (const path of paths) {
    if (path.toLowerCase().endsWith(".pdf")) pdfPaths.push(path);
    else imagePaths.push(path);
  }
  if (pdfPaths.length > 0) void addPdfPaths(pdfPaths);
  if (imagePaths.length > 0) void addImagePaths(imagePaths);
}

/** S1: the preview's own undo — the image simply was not sent. */
function handleRemoveImage(id: string): void {
  removeAttachment(draftKey.value, id);
}

/**
 * The buffer of the session on screen. Everything below — transcript, streaming
 * tail, tools, status — is read from here, which is why selecting another
 * session is instant and why a session that was off screen still shows its
 * history when it comes back (S4).
 */
/** Compact 3-word session title for the topbar breadcrumb beside the workspace. */
const sessionHeading = computed<string | null>(() => {
  const raw =
    activeSessionTitle.value?.trim() ||
    transcript.value.find((m) => m.role === "user")?.text.trim() ||
    "";
  if (raw === "") return null;
  const words = raw.replace(/\s+/g, " ").split(" ");
  if (words.length <= 3) return words.join(" ");
  return `${words.slice(0, 3).join(" ")}…`;
});

const buffer = computed<SessionBuffer | null>(() =>
  sessionBuffer(currentSession.value?.sessionId ?? null),
);
const transcript = computed(() => buffer.value?.transcript ?? []);
/**
 * Every tool call of the session, oldest first, each one stamped with the human
 * turn it was made for (`ToolEntry.turnId`) and with the step it ran in
 * (`ToolEntry.stepId`). No longer cleared per turn: that is what lets a finished
 * turn keep its chips inside its own disclosure.
 */
const tools = computed<ToolEntry[]>(() => buffer.value?.tools ?? []);

/**
 * The turn the store is answering right now, or `null` when the session is idle.
 *
 * It is the id of the human message that opened it (`SessionBuffer.turnId`), and
 * `busy` is what says the turn has not ended: at `turn_end` everything folds and
 * the answer is the last thing left out of the fold (S3).
 */
const liveTurnId = computed<string | null>(() => {
  const current = buffer.value;
  if (current === null || !current.busy) return null;
  return current.turnId;
});

/**
 * The conversation, grouped by human turn (S1): one group per human message,
 * holding the steps pi took for it, its chips and its final answer.
 *
 * `turn-groups.ts` owns the derivation, including the difference between a live
 * turn (attribution stamped by the store) and a transcript read back from pi's
 * file (grouped by message order, since nothing about it is persisted).
 */
const turnGroups = computed(() =>
  deriveTurnGroups(transcript.value, tools.value, liveTurnId.value),
);

/**
 * How much of the conversation is mounted: the newest page of turns, plus the
 * pages the human asked for (`turn-window.ts`). A session that has run for hours
 * is not mounted whole to show its last answer, and the window resets when the
 * session on screen changes — one conversation's reveals say nothing about
 * another's.
 *
 * The identity is read from `currentSession` rather than `activeSessionId`
 * because that derived value is declared further down: a watch getter runs during
 * setup, and referencing a later `const` is a temporal-dead-zone crash.
 */
const {
  groups: visibleTurnGroups,
  hiddenCount: hiddenTurnCount,
  hasOlder: hasOlderTurns,
  showOlder: revealOlderTurns,
} = useTurnWindow(turnGroups, () => currentSession.value?.sessionId ?? null);
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
 * H4 of `odd/tasks/subagent-helpers.md`: the panel is showing the read-only view of
 * a child instead of a conversation.
 *
 * The child view is a *state of this column*, not a page and not a second panel:
 * the conversation underneath was never left (its buffer, its process and its
 * draft are untouched), so going back is a reset of one ref in the shared store.
 */
const showsSubagentView = computed(() => openSubagent.value !== null);

/** The child view's back control: returns the column to the chat it covered. */
function handleCloseSubagent(): void {
  closeSubagentView();
}

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
 * Scroll behaviour of the transcript: one controller
 * (`transcript-follow.ts`) owns the pin, the coalesced scroll and the anchor used
 * when older turns are revealed above the reader. It follows the newest line only
 * while the human is already there, so reading the middle of a conversation
 * survives the tokens that keep streaming in (and any other growth of the
 * transcript).
 */
const follow = createTranscriptFollow();
/** The transcript's content wrapper; the scrolling element is its ancestor. */
const transcriptContent = ref<HTMLElement | null>(null);
/** The element the scroll listener is bound to, so detaching can unbind it. */
let transcriptViewport: HTMLElement | null = null;
let transcriptResize: ResizeObserver | null = null;

function detachTranscriptViewport(): void {
  if (transcriptViewport !== null) {
    transcriptViewport.removeEventListener("scroll", handleTranscriptScroll);
    transcriptViewport = null;
  }
  transcriptResize?.disconnect();
  transcriptResize = null;
  // `detach` cancels a frame that has not landed yet: nothing scrolls a
  // transcript that is no longer on screen.
  follow.detach();
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
  // Opening at the end belongs to `attach`: a freshly mounted transcript has no
  // position to preserve.
  follow.attach(viewport);
  transcriptViewport = viewport;
  if (viewport === null) return;
  viewport.addEventListener("scroll", handleTranscriptScroll, { passive: true });
  transcriptResize ??= new ResizeObserver(handleTranscriptResize);
  transcriptResize.observe(viewport);
  // The content element is observed too: collapsing the reasoning block (or a
  // code block growing) changes its height without touching the viewport, and a
  // view glued to the bottom must follow that change as well.
  if (content !== null) transcriptResize.observe(content);
}

/** Every movement of the viewport, whoever made it (wheel, keys, scrollbar, touch). */
function handleTranscriptScroll(): void {
  follow.handleScroll();
}

/**
 * A resize (window, drawer, composer) reflows the transcript: a view glued to the
 * bottom is put back on it, a free one only re-reads its pin.
 */
function handleTranscriptResize(): void {
  follow.handleResize();
}

/**
 * Reveals one page of older turns, keeping the reader exactly where they were:
 * the new turns land *above* them, so the offset has to absorb that growth
 * (`TranscriptFollow.restore`). The control only exists at the top of the mounted
 * window, which is where a reader who walks back through history already is.
 */
function handleShowOlderTurns(): void {
  const anchor = follow.snapshot();
  revealOlderTurns();
  void nextTick(() => follow.restore(anchor));
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

/**
 * S2 of `odd/tasks/composer-image-input.md`: the input modalities of the model the
 * composer names, as pi's own catalogue reports them for the persisted pair.
 * `null` while the catalogue does not answer, which is not evidence either way.
 */
const selectedModelModalities = computed<string[] | null>(() => {
  const provider = providerModels.value.find(
    (entry) => entry.provider === providerId.value,
  );
  const model = provider?.models.find((entry) => entry.id === modelId.value);
  return model?.modalities ?? null;
});

/**
 * S2: the images cannot be sent as things stand, and the reason is explained
 * before anything leaves. Never a model switch: the human chooses the model, and
 * this panel only refuses to pretend.
 *
 * `null` modalities means the catalogue has not answered yet; the authoritative
 * check is Rust's (the live model's own `input` list), and this notice would
 * otherwise flash on every mount.
 */
const imageInputUnsupported = computed(
  () =>
    images.value.length > 0 &&
    selectedModelModalities.value !== null &&
    !selectedModelModalities.value.includes("image"),
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
const selectedModelContextWindow = computed<number | null>(() => {
  const provider = providerModels.value.find(
    (entry) => entry.provider === providerId.value,
  );
  const model = provider?.models.find((entry) => entry.id === modelId.value);
  return model?.contextWindow ?? null;
});

const estimatedContextTokens = computed(() =>
  estimateTokens([
    ...transcript.value.map((m) => m.text),
    streamingText.value,
  ]),
);

const contextRatio = computed(() =>
  contextUsageRatio(
    estimatedContextTokens.value,
    selectedModelContextWindow.value,
  ),
);

const contextLabel = computed<string | null>(() => {
  if (transcript.value.length === 0 && streamingText.value === "") return null;
  const used = formatCompactTokens(estimatedContextTokens.value);
  const limit = selectedModelContextWindow.value;
  return limit ? `~${used} / ${formatCompactTokens(limit)}` : `~${used} tok`;
});

const contextHigh = computed(
  () => contextRatio.value !== null && contextRatio.value >= 0.75,
);

const exportedMarkdown = ref(false);
let exportTimer: number | null = null;
async function handleExportMarkdown(): Promise<void> {
  const md = formatTranscriptMarkdown(
    workspaceName.value,
    activeSessionTitle.value,
    transcript.value,
  );
  try {
    await navigator.clipboard.writeText(md);
    exportedMarkdown.value = true;
    if (exportTimer !== null) window.clearTimeout(exportTimer);
    exportTimer = window.setTimeout(() => {
      exportedMarkdown.value = false;
      exportTimer = null;
    }, 1800);
  } catch {
    exportedMarkdown.value = false;
  }
}

const turnStartedAt = ref<number | null>(null);
const turnElapsedSecs = ref(0);
let elapsedTimer: number | null = null;
const turnElapsedLabel = computed<string | null>(() =>
  turnStartedAt.value !== null ? formatElapsed(turnElapsedSecs.value) : null,
);

const agentWorking = computed(
  () => sending.value || starting.value || (buffer.value?.busy ?? false),
);

/**
 * Human-readable description of what the agent is doing right now, rendered
 * directly at the bottom of the transcript so the user never stares at static
 * text wondering whether the session is alive.
 */
const liveActivityLabel = computed<string>(() => {
  const activeChildren = subagentChildrenOf(
    currentSession.value?.sessionId ?? null,
  ).filter((c) => isSubagentActive(c.status));
  if (activeChildren.length > 0) {
    const names = activeChildren
      .map((c) => c.agent.trim() || c.name)
      .join(", ");
    return t("chat.activity.subagent", { names });
  }
  for (let i = tools.value.length - 1; i >= 0; i -= 1) {
    const entry = tools.value[i];
    if (entry.state === "running") {
      const cleanName = entry.toolName.replace(/^mcp__[^_]+__/, "");
      return t("chat.activity.tool", { tool: cleanName });
    }
  }
  if (streamingReasoning.value !== "" && streamingText.value === "") {
    return t("chat.activity.thinking");
  }
  if (streamingText.value !== "") {
    return t("chat.activity.writing");
  }
  return t("chat.activity.waitingModel");
});

watch(agentWorking, (working, wasWorking) => {
  if (working) {
    if (turnStartedAt.value === null) {
      turnStartedAt.value = Date.now();
      turnElapsedSecs.value = 0;
    }
    if (elapsedTimer === null) {
      elapsedTimer = window.setInterval(() => {
        if (turnStartedAt.value !== null) {
          turnElapsedSecs.value = Math.floor(
            (Date.now() - turnStartedAt.value) / 1000,
          );
        }
      }, 1000);
    }
  } else {
    turnStartedAt.value = null;
    if (elapsedTimer !== null) {
      window.clearInterval(elapsedTimer);
      elapsedTimer = null;
    }
    // Configurable OS notification when a background turn completes.
    if (wasWorking) {
      void sendAppNotification(
        "turnFinish",
        t("notify.turnFinished.title"),
        t("notify.turnFinished.body", { workspace: workspaceName.value }),
      );
    }
  }
});

/**
 * The prompt can be sent right now: there is a workspace, something to say, and
 * the agent is not working. "Not working" reads `agentWorking`, not a private
 * copy of it, so a submit and the stop control can never disagree about whether
 * the turn is over.
 */
const canSend = computed(
  () =>
    props.workspace !== null &&
    (prompt.value.trim() !== "" ||
      images.value.length > 0 ||
      pdfs.value.length > 0) &&
    !imageInputUnsupported.value &&
    !pdfUnreadable.value &&
    !agentWorking.value,
);

/**
 * The composer holds the text of the send that is in flight, not a new prompt.
 *
 * Declared before the computeds that read it, like every other state in this
 * panel: `canQueue` and `handleQueue` both consult it, so the control and the
 * handler cannot disagree about whether this text has already left.
 */
const submittingSamePrompt = computed(
  () =>
    inFlightPrompt.value !== null &&
    prompt.value.trim() === inFlightPrompt.value &&
    images.value.length === inFlightImages.value &&
    pdfs.value.length === inFlightPdfs.value &&
    intents.value.length === inFlightIntents.value,
);

/**
 * S3: the prompt can be queued instead of sent — the agent is working, the
 * preference is on, there is a session to queue it against and there is
 * something to queue. The store keeps the queue per session; the panel only
 * decides which event the composer emits.
 *
 * The in-flight send is handled by OWNERSHIP ([`submittingSamePrompt`]), not by
 * the latch: a prompt typed after the click is a different text and belongs in the
 * queue even while the send's round-trip is still open. Refusing every submit
 * during that window was the first-click drop — the control said "queue", the
 * latch said no, and only the click after the round-trip did anything.
 */
const canQueue = computed(
  () =>
    queueMessages.value &&
    agentWorking.value &&
    !submittingSamePrompt.value &&
    currentSession.value !== null &&
    props.workspace !== null &&
    (prompt.value.trim() !== "" ||
      images.value.length > 0 ||
      pdfs.value.length > 0) &&
    !imageInputUnsupported.value &&
    !pdfUnreadable.value,
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
 * store owns the list — it settles every one of them when the process stops — so
 * the panel only renders one at a time and answers it.
 */
const questions = computed(() =>
  pendingQuestions(currentSession.value?.sessionId ?? null),
);

/**
 * Id of the question whose answer is on its way. While set, the question's
 * controls are inert: a second answer would write a second response for a request
 * pi has already resolved, and the protocol has no way to take one back.
 */
const answering = ref<string | null>(null);

/**
 * Which pending question holds the composer slot. `null` means "the oldest one":
 * questions are answered in the order they were asked unless the human walks the
 * stack with Tab, and an id whose question already left falls back to the oldest
 * remaining instead of leaving the slot empty.
 */
const focusedQuestionId = ref<string | null>(null);

/**
 * What has been typed for each pending question, keyed by its request id. The
 * text belongs to the question that asked for it, so walking the stack with Tab
 * never shows one question the answer typed for another.
 */
const answerDrafts = reactive(new Map<string, string>());

/** Position of the question on screen in the pending stack; `-1` when none. */
const focusedQuestionIndex = computed(() => {
  const list = questions.value;
  if (list.length === 0) return -1;
  const index = list.findIndex(
    (question) => question.id === focusedQuestionId.value,
  );
  return index === -1 ? 0 : index;
});

/** The question the composer slot is asking right now, or `null`. */
const focusedQuestion = computed<PendingQuestion | null>(
  () => questions.value[focusedQuestionIndex.value] ?? null,
);

/**
 * The answer field's text (`v-model` of the question prompt): the draft of the
 * question on screen, and only that one. As a computed it needs no resetting —
 * moving the focus is enough for the field to show the right text.
 */
const answerText = computed<string>({
  get: () => {
    const id = focusedQuestion.value?.id;
    return id === undefined ? "" : (answerDrafts.get(id) ?? "");
  },
  set: (value) => {
    const id = focusedQuestion.value?.id;
    if (id === undefined) return;
    if (value === "") answerDrafts.delete(id);
    else answerDrafts.set(id, value);
  },
});

/**
 * The block that hosts the composer, so focus can be put back in its field by a
 * lookup instead of a component contract: the composer is a fresh mount each time
 * the slot stops asking a question, so an instance handle is less reliable here
 * than the element itself.
 */
const composerSlot = ref<HTMLElement | null>(null);

/**
 * Forgets the draft of every question that is no longer pending, and nothing else.
 * A settled question takes its half-typed text with it: the request is closed, so
 * keeping the text would only resurrect it on a request id that never returns.
 */
watch(questions, (list, previous) => {
  const live = new Set(list.map((question) => question.id));
  for (const id of [...answerDrafts.keys()]) {
    if (!live.has(id)) answerDrafts.delete(id);
  }
  const prevIds = new Set((previous ?? []).map((question) => question.id));
  if (list.some((question) => !prevIds.has(question.id))) {
    void sendAppNotification(
      "question",
      t("notify.question.title"),
      t("notify.question.body", { workspace: workspaceName.value }),
    );
  }
});

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
/** Cumulative for the session: the chips are no longer dropped per turn, so
 * "steps so far" keeps counting the whole conversation. */
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
    // A question that arrives grows the transcript too: it renders at the end of
    // the flow (S3), so a view glued to the bottom must follow it there.
    questions.value.length,
    errorText.value?.length ?? 0,
    statusAlert.value?.text ?? "",
  ].join("|"),
);

/**
 * The follow itself: while the pin holds, every growth moves the view to the new
 * bottom; while it does not, nothing touches the scroll offset, so a human
 * reading higher up keeps the exact place they had. The scroll is coalesced to
 * one per frame (`request`), because several growths — a committed turn and its
 * chips, the throttled tail and the reasoning — land in the same one.
 */
watch(
  transcriptSignature,
  () => {
    follow.request();
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
  follow.jump();
  void nextTick(() => {
    // The history may still be loading; only keep following when we truly landed
    // at the end (a short conversation leaves nothing to scroll).
    follow.syncPin();
  });
});

/**
 * Point 5 of the design: while a question holds the composer slot the composer is
 * not rendered, so "focus returns to the input" is a focus call and not a browser
 * default. It runs when the last question leaves — answered, cancelled, or
 * settled as cancelled by a stop — and never while one is still asking, which is
 * what keeps an `ask_input` field from being interrupted mid-word.
 *
 * The session id is watched with it because switching sessions changes the count
 * too, and the human's focus is in the drawer then: taking it into a composer
 * they did not ask for is not this behaviour.
 *
 * It lives here, after `activeSessionId`'s declaration, on purpose: a watch's
 * getter runs once during setup, so referencing a `const` that is declared later
 * is a temporal-dead-zone crash that blanks the whole panel — the blank-screen
 * regression this block shipped with taught the file that.
 */
watch(
  () => [activeSessionId.value, questions.value.length] as const,
  ([session, next], [previousSession, previous]) => {
    if (session !== previousSession) return;
    if (next > 0 || previous === 0) return;
    void nextTick(() => composerSlot.value?.querySelector("textarea")?.focus());
  },
);

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
    // S2 of `drawer-responsiveness-oauth.md`: the startup load reuses the payload
    // `main.ts` already read for the language, instead of asking the file a second
    // time before the first paint. `takeBootSettings()` answers `null` once, and
    // this is the only caller — every other settings read in this panel stays a
    // fresh command, because by then the file may have changed.
    settings.value = takeBootSettings() ?? (await getSettings());
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
 * file). That write is the default the NEXT session spawns with.
 *
 * A session that is already live is switched in place, through pi's own
 * `set_model`: persisting alone would leave the running child on the model the
 * human just left, with the effort list and the agent's reasoning still belonging
 * to it. The level pi re-clamps after the switch reaches the effort selector as
 * `thinking_level_changed`, so nothing here touches the level.
 *
 * The liveness check is the store's own hint (the runtime's last word), so a
 * session that is spawning or gone takes the persist-only path and reports
 * nothing: the choice is waiting for the next spawn. A live child that refuses
 * the switch does surface it, because nothing was switched.
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

    const session = currentSession.value;
    if (session === null || !liveIds.value.includes(session.sessionId)) return;
    try {
      const live = await setSessionModel(
        session.workspaceId,
        Number(session.sessionId),
        choice.provider,
        choice.model,
      );
      // pi's answer, not this panel's echo: an alias becomes the model it named,
      // and that is the honest default for the next session too (the file is
      // re-read first, exactly like the write above).
      if (live.provider !== choice.provider || live.id !== choice.model) {
        settings.value = await saveSettings({
          ...(await getSettings()),
          provider: live.provider,
          model: live.id,
        });
      }
      // The effort follows the model. pi's clamp never lifts "off" — it is a
      // level every model supports — so a session that started on a
      // non-reasoning model would carry "off" onto a model that reasons, and
      // the human would find their agent muted without touching anything.
      // Re-applying the persisted preference right after the switch lets pi
      // clamp it to the new model's own range: the preference follows the
      // model, and "off" survives only where the model truly has no reasoning.
      await setSessionThinkingLevel(
        session.workspaceId,
        Number(session.sessionId),
        current.thinkingLevel,
      );
    } catch (cause) {
      panelError.value = commandErrorMessage(cause);
    }
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
  // S3 (point 6 of the design): while a question holds the composer slot, absent
  // a picker there is no prompt to send — the slot is busy being the answer. A
  // typed question is answered from the same Enter that would otherwise send; the
  // picker has no field, so Enter there is the highlighted option's own and this
  // only refuses to invent a send.
  const question = focusedQuestion.value;
  if (question !== null) {
    if (asksForText(question) && answerText.value.trim() !== "") {
      void handleAnswer(question, { value: answerText.value.trim() });
    }
    return;
  }
  const text = prompt.value.trim();
  // S1-S2: the images of this draft travel with the text, and an image-only turn
  // is a turn. The wire copy is taken now, before a send can clear the draft.
  const attached = attachmentsToPromptImages(draftKey.value);
  // S2-S3 of `composer-tools-pdf.md`: the text the agent receives is composed
  // once here — question, requested tools and page-numbered PDFs — while the
  // label the transcript keeps names the attachments and never their content.
  const turn = {
    imageCount: attached.length,
    pdfs: pdfs.value,
    intents: intents.value,
  };
  const wireText = composeTurnPrompt(prompt.value, turn);
  const display = composeTurnDisplay(prompt.value, turn);
  if (text === "" && attached.length === 0 && pdfs.value.length === 0) return;
  if (imageInputUnsupported.value) {
    debugLog("chat send: images attached but the named model takes none");
    return;
  }
  if (pdfUnreadable.value) {
    debugLog("chat send: a PDF has no selectable text (scan), nothing sent");
    return;
  }
  // The key the text was typed under, before a send can create its session.
  const originKey = draftKey.value;
  // S3: with the queue preference on, a prompt typed while the agent works is
  // queued instead of dropped. The guard used to return here for both cases,
  // which silently discarded what the human had just typed — the toggle said
  // "Encolar mensajes" and this path ignored it. The queue is the same one the
  // explicit action fills, so a queued prompt looks and drains the same way.
  if (agentWorking.value) {
    debugLog(
      `chat send: agent working, queue pref=${queueMessages.value} canQueue=${canQueue.value}`,
    );
    if (queueMessages.value) handleQueue();
    return;
  }
  sending.value = true;
  // From here until pi accepts it, this text belongs to the send: a submit of the
  // SAME text must not queue it a second time, and a different one may.
  inFlightPrompt.value = text;
  inFlightImages.value = attached.length;
  inFlightPdfs.value = turn.pdfs.length;
  inFlightIntents.value = turn.intents.length;
  panelError.value = null;
  try {
    const current = await ensureSession();
    if (current === null) return;
    // The session exists now, so the draft (and its images) live under its key.
    // Moving them keeps the previews on screen while the send is in flight and
    // keeps a rejected send's images with the text that is still in the composer.
    moveAttachments(originKey, draftKey.value);
    if (isSessionSpawning(current.sessionId)) {
      enqueuePrompt(current.sessionId, wireText, current.workspaceId, attached, display);
      forgetSentDraft(originKey);
      return;
    }
    await sendUserTurn(
      current.sessionId,
      current.workspaceId,
      wireText,
      attached,
      display,
    );
    forgetSentDraft(originKey);
  } catch (cause) {
    // The composer keeps the text and the images: a prompt that never left is
    // still the human's to resend.
    panelError.value = commandErrorMessage(cause);
  } finally {
    sending.value = false;
    inFlightPrompt.value = null;
    inFlightImages.value = 0;
    inFlightPdfs.value = 0;
    inFlightIntents.value = 0;
  }
}

/**
 * Forgets a draft that is no longer unsent (sent or queued), including its
 * pending images. The extra key covers the first send of a workspace: its session
 * was created during the send, so the text and the images were typed under the
 * workspace key and now live under the session's.
 */
function forgetSentDraft(originKey: string | null): void {
  const sessionKey = draftKey.value;
  draft.clear(originKey);
  clearAttachments(originKey);
  clearAttachments(sessionKey);
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
  const attached = attachmentsToPromptImages(draftKey.value);
  // S2-S3: the same composition the direct send uses, so a queued turn carries
  // exactly what a sent one would — one line per requested tool, the PDFs with
  // their page numbers, and a label that names them without copying their text.
  const turn = {
    imageCount: attached.length,
    pdfs: pdfs.value,
    intents: intents.value,
  };
  const wireText = composeTurnPrompt(prompt.value, turn);
  const display = composeTurnDisplay(prompt.value, turn);
  // Ownership, not the latch (see `canQueue`): the text of the send that is in
  // flight is already on its way and must not also enter the queue; a prompt
  // typed after that click is somebody else's text and is queued now.
  if (submittingSamePrompt.value) {
    debugLog("chat queue: ignored, this text belongs to the in-flight send");
    return;
  }
  if (
    current === null ||
    (text === "" && attached.length === 0 && pdfs.value.length === 0) ||
    !queueMessages.value ||
    imageInputUnsupported.value ||
    pdfUnreadable.value
  ) {
    debugLog(
      `chat queue: nothing to queue (session=${current !== null} text=${text !== ""} images=${attached.length} pdfs=${pdfs.value.length} pref=${queueMessages.value})`,
    );
    return;
  }
  // The panel knows the workspace of the session it renders, so the queued
  // prompt is addressable the moment the turn ends — even if the human has
  // moved to another workspace by then.
  enqueuePrompt(current.sessionId, wireText, current.workspaceId, attached, display);
  // P2: a queued prompt is not unsent either, so the session's draft goes too.
  draft.clear();
  clearAttachments(draftKey.value);
  debugLog(
    `chat queue: enqueued, ${queuedCount(current.sessionId)} waiting in this session`,
  );
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
 * S5 of `turn-continuity.md`: continues the turn the human stopped (or one that
 * was cut short), after the notice inside that turn.
 *
 * This is a *new* turn, not a re-run of the previous prompt: the message says the
 * previous turn did not finish and asks the agent to pick up where it stopped, so
 * the transcript shows what was asked instead of silently repeating work the
 * agent already did. It goes through `sendUserTurn`, which is the only path a
 * human turn is recorded by — and whose own re-open path covers a session whose
 * pi is no longer live, so there is no second way to reach a child here.
 *
 * Guarded like every other send: one prompt at a time, and never while the agent
 * is still working (the notice only exists on a turn that already ended).
 */
async function handleContinue(): Promise<void> {
  const current = currentSession.value;
  if (current === null) return;
  if (agentWorking.value || sending.value || starting.value) return;
  sending.value = true;
  panelError.value = null;
  try {
    await sendUserTurn(
      current.sessionId,
      current.workspaceId,
      t("chat.turn.continuePrompt"),
    );
  } catch (cause) {
    panelError.value = commandErrorMessage(cause);
  } finally {
    sending.value = false;
  }
}

/**
 * S3: answers one pending question, which is what unblocks the agent's turn.
 *
 * The question leaves the composer slot only once the line really reached the
 * child (`answer_ui_request` rejects for a session that is not running), so a
 * question that is gone always means the agent has the answer. A failure keeps it
 * on screen with the reason in the error card, which is the honest state: still
 * unanswered.
 *
 * Settling is part of the same step, and the store owns it: the answer travels
 * with the removal (`resolveQuestion`) so the transcript gains the exchange in the
 * moment the pending stack loses it.
 */
async function handleAnswer(question: PendingQuestion, answer: UiAnswer) {
  const current = currentSession.value;
  if (current === null || answering.value !== null) return;
  answering.value = question.id;
  panelError.value = null;
  // Where the question stood, read before the store removes it.
  const index = focusedQuestionIndex.value;
  try {
    await answerUiRequest(
      current.workspaceId,
      Number(current.sessionId),
      question.id,
      answer,
    );
    resolveQuestion(current.sessionId, question.id, answer);
    advanceFocus(index);
  } catch (cause) {
    panelError.value = commandErrorMessage(cause);
  } finally {
    answering.value = null;
  }
}

/**
 * S3: after an answer, focus advances to the question that followed the one just
 * answered — or to the last one left when it was the last. `index` is where that
 * question stood before the store removed it.
 */
function advanceFocus(index: number): void {
  const remaining = questions.value;
  if (remaining.length === 0) {
    focusedQuestionId.value = null;
    return;
  }
  const next = Math.max(0, Math.min(index, remaining.length - 1));
  focusedQuestionId.value = remaining[next]?.id ?? null;
}

/**
 * S3: Tab / Shift+Tab walk the pending questions of this session. Only meaningful
 * with more than one, and wrapping at both ends, because a stack of two is a ring
 * the human moves around rather than a list with edges.
 */
function focusQuestionByOffset(offset: number): void {
  const list = questions.value;
  if (list.length < 2) return;
  const index =
    (focusedQuestionIndex.value + offset + list.length) % list.length;
  focusedQuestionId.value = list[index]?.id ?? null;
}

/**
 * S3: cancels the question on screen — the visible half of Escape. It goes through
 * the same command as an answer, with the protocol's cancellation, so pi gets its
 * turn back and the question settles into the transcript as cancelled.
 */
function handleCancelQuestion(): void {
  const question = focusedQuestion.value;
  if (question === null) return;
  void handleAnswer(question, { cancelled: true });
}

/** S3: the answer the question on screen is waiting for. */
function handleQuestionAnswer(answer: UiAnswer): void {
  const question = focusedQuestion.value;
  if (question === null) return;
  void handleAnswer(question, answer);
}

/**
 * The question on screen is answered by typing rather than by picking a row. The
 * picker has no field, so this is also what tells `handleSend` that Enter means
 * "answer" instead of "send a prompt".
 */
function asksForText(question: PendingQuestion): boolean {
  return question.options.length === 0 && question.method !== "confirm";
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
  // S2: whether this run bound the MCP endpoint the agent's tools come from.
  // Read once — it is a property of the process, not of the turn — and a failure
  // answers "no tools", never a promise.
  void mcpEndpoint()
    .then((endpoint) => {
      toolEndpointReady.value = endpoint !== null;
    })
    .catch(() => {
      toolEndpointReady.value = false;
    });
  // L3/S4: the store owns the single `agent-event` subscription and buffers
  // every session, so this panel only asks for it to be installed. It is never
  // torn down with the panel: a background session must keep receiving events.
  void ensureSessionEvents();
});

onUnmounted(() => {
  // Only the focus listener and the transcript listeners are ours: the
  // agent-event subscription belongs to the store and must survive a panel swap.
  window.removeEventListener("focus", handleWindowFocus);
  if (elapsedTimer !== null) {
    window.clearInterval(elapsedTimer);
    elapsedTimer = null;
  }
  detachTranscriptViewport();
});
</script>

<template>
  <div class="relative flex h-full min-h-0 flex-col">
    <!-- Corner cluster for the no-conversation state: the header (which carries
         the same controls) does not exist yet, so both switches live here until
         the first turn. Tools toggle hides while the drawer is open.
         -->
    <div
      v-if="isEmptyState && !showsSubagentView"
      class="absolute top-2 right-2 z-10 flex items-center gap-1"
    >
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

    <!-- H4 of `odd/tasks/subagent-helpers.md`: the read-only view of a child a chat
         delegated to. It takes the column before either chat state does — a view
         opened from the sidebar must show even when the chat behind it has nothing
         painted yet — and it owns its whole surface: no composer, no queue, no
         picker, and no pi spawn path at all. -->
    <SubagentView v-if="showsSubagentView" @back="handleCloseSubagent" />

    <!-- S1: no conversation yet — brand mark, chips and composer, no header. -->
    <ChatEmptyState
      v-else-if="isEmptyState"
      :provider="providerName"
      :hint="workspace === null ? t('chat.noWorkspaceHint') : null"
    >
      <template #composer>
        <!-- `composerSlot`: where focus goes back to when the last pending
             question leaves (S3, point 5 of the design). -->
        <div ref="composerSlot" class="flex w-full flex-col gap-1.5">
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

          <!-- S3 of `agent-question-bridge.md`: a pending question takes the
               composer's place, not the transcript's — the answer belongs where
               the human's hands already are. The queue keeps its strip above it:
               what waits for the next turn is still visible, and still
               removable, while the question owns the input row. -->
          <QuestionPrompt
            v-if="focusedQuestion"
            :key="focusedQuestion.id"
            v-model="answerText"
            :question="focusedQuestion"
            :position="focusedQuestionIndex + 1"
            :total="questions.length"
            :busy="answering !== null"
            :max-width="COMPOSER_MAX_WIDTH"
            @answer="handleQuestionAnswer"
            @cancel="handleCancelQuestion"
            @next="focusQuestionByOffset(1)"
            @previous="focusQuestionByOffset(-1)"
          />
          <Composer
            v-else
            v-model="prompt"
            :max-width="COMPOSER_MAX_WIDTH"
            :provider-id="providerId"
            :model-id="modelId"
            :provider-models="providerModels"
            :workspace-name="workspace?.name ?? t('chat.noWorkspace')"
          :workspace-id="workspace?.id ?? null"
            :streaming="agentWorking"
            :disabled="workspace === null"
            :can-send="canSend"
            :can-queue="canQueue"
            :queued-count="queuedForSession"
            :model-disabled="savingModel || enabledModelCount === 0"
            :can-stop="currentSession !== null && !stopping"
            :images="images"
            :image-input-unsupported="imageInputUnsupported"
            :pdfs="pdfs"
            :intents="intents"
            :pdf-unreadable="pdfUnreadable"
            :pdf-reading="pdfReading > 0"
            :web-search-enabled="webSearchEnabled"
            :tool-endpoint-ready="toolEndpointReady"
            :browser-enabled="browserToolEnabled"
            @select-model="handleSelectModel"
            @send="handleSend"
            @queue="handleQueue"
            @stop="handleStop"
            @pick-images="handlePickImages"
            @remove-image="handleRemoveImage"
            @paste-images="handlePasteImages"
            @drop-files="handleDropFiles"
            @pick-pdf="handlePickPdf"
            @remove-pdf="handleRemovePdf"
            @toggle-intent="handleToggleIntent"
          />
        </div>
      </template>
    </ChatEmptyState>

    <!-- S2: conversation state — header, transcript, composer. -->
    <template v-else>
      <ChatHeader
        :title="workspaceName"
        :session-title="sessionHeading"
        :working="agentWorking"
        :elapsed="turnElapsedLabel"
        :context-label="contextLabel"
        :context-high="contextHigh"
        :exported="exportedMarkdown"
        :tools-open="toolsOpen"
        :is-dark="isDark"
        @toggle-tools="emit('toggleTools')"
        @toggle-theme="handleToggleTheme"
        @open-search="emit('openSearch')"
        @export-markdown="handleExportMarkdown"
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
          class="mx-auto flex w-full flex-col gap-5 px-6 py-6"
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

          <!-- L3/S4: the active session's buffer, grouped by human turn (S1 of
               `chat-turn-groups.md`): one group per human message, holding the
               steps pi took for it inside a single disclosure and its final
               answer visible under it (S2). A turn still running keeps the step
               in progress outside that fold (S3).

               Only the newest page is mounted (`turn-window.ts`); the control
               above reveals the rest, one page at a time. -->
          <OlderTurnsControl
            v-if="hasOlderTurns"
            :count="hiddenTurnCount"
            @reveal="handleShowOlderTurns"
          />

          <TurnGroup
            v-for="group in visibleTurnGroups"
            :key="group.id"
            :turn-id="group.id"
            :prompt="group.prompt"
            :at="group.prompt === null ? undefined : timeLabel(group.prompt.at)"
            :question="group.question"
            :steps="group.steps"
            :tools="group.tools"
            :answer="group.answer"
            :live="group.live"
            :interruption="group.interruption"
            :live-text="liveText"
            :live-reasoning="streamingReasoning"
            :live-message-id="liveMessageId"
            :live-tools="group.liveTools"
            :show-reasoning="showReasoning"
            :parent-session-id="currentSession?.sessionId ?? null"
            :workspace-id="workspace?.id ?? null"
            @continue="handleContinue"
          />

          <!-- S3 of `agent-question-bridge.md`: a question the agent is blocked
               on is part of the conversation while it waits, not a surface that
               appears from nowhere. It renders right after the last turn —
               which is where the newest thing a blocked turn has produced sits,
               because nothing streams after the tool call that asked — and in
               arrival order when pi asked several. Answering it swaps this
               pending card for the settled one in the very same slot: the entry
               `resolveQuestion` appends becomes the next turn group above, so the
               exchange never jumps down the transcript and never appears twice. -->
          <QuestionCard
            v-for="question in questions"
            :key="question.id"
            :question="question"
          />

          <!-- Live activity bar pinned at the bottom of the transcript while
               the agent is working (especially helpful during long tool calls,
               subagent runs, or model thinking pauses). -->
          <div
            v-if="agentWorking && questions.length === 0"
            role="status"
            aria-live="polite"
            class="flex items-center gap-2.5 rounded-xl border border-primary/25 bg-primary/5 px-3.5 py-2.5 text-xs text-foreground"
          >
            <LoaderCircleIcon
              class="size-3.5 shrink-0 animate-spin text-primary"
              aria-hidden="true"
            />
            <span class="min-w-0 flex-1 truncate font-medium">
              {{ liveActivityLabel }}
            </span>
            <span
              v-if="turnElapsedLabel"
              class="shrink-0 font-mono text-[11px] tabular-nums text-muted-foreground"
            >
              {{ turnElapsedLabel }}
            </span>
          </div>
        </div>
      </ScrollArea>

      <!-- Composer, pinned under the transcript, plus the turns/steps meta. -->
      <div
        ref="composerSlot"
        class="bg-background px-5 pt-2 pb-3.5"
      >
        <!-- S3: this session's waiting prompts, directly above the composer and
             capped to three visible rows so the input never leaves the screen.
             Same stack as the empty state above. -->
        <div
          v-if="queuedList.length > 0"
          class="mx-auto mb-2.5 flex w-full flex-col overflow-hidden rounded-xl border border-border bg-card"
          :style="{ maxWidth: queueMaxWidth }"
        >
          <p class="border-b border-sidebar-border px-3 py-2 text-[11px] font-semibold text-foreground">
            {{ t("chat.queue.heading", { count: queuedList.length }) }}
          </p>
          <ul
            class="max-h-[5.5rem] overflow-y-auto p-1"
            :aria-label="t('chat.queue.list')"
          >
            <li
              v-for="(item, index) in queuedList"
              :key="`${index}-${item}`"
              class="flex items-center gap-2 rounded-md px-2 py-1 hover:bg-secondary"
            >
              <span
                class="w-4 shrink-0 text-right font-mono text-[10px] tabular-nums text-muted-foreground"
                aria-hidden="true"
              >
                {{ index + 1 }}
              </span>
              <span class="min-w-0 flex-1 truncate text-xs text-foreground">
                {{ item }}
              </span>
              <Button
                variant="ghost"
                size="icon-xs"
                class="shrink-0 text-muted-foreground hover:text-foreground"
                :aria-label="t('chat.queue.remove', { index: index + 1 })"
                @click="handleRemoveQueued(index)"
              >
                <XIcon />
              </Button>
            </li>
          </ul>
        </div>

        <!-- S3 of `agent-question-bridge.md`: the question takes the composer's
             place, and the queue keeps its strip above it — what waits for the
             next turn stays visible, and removable, while the question owns the
             input row. Same order as the empty state above. -->
        <QuestionPrompt
          v-if="focusedQuestion"
          :key="focusedQuestion.id"
          v-model="answerText"
          :question="focusedQuestion"
          :position="focusedQuestionIndex + 1"
          :total="questions.length"
          :busy="answering !== null"
          :max-width="CONVERSATION_COMPOSER_MAX_WIDTH"
          @answer="handleQuestionAnswer"
          @cancel="handleCancelQuestion"
          @next="focusQuestionByOffset(1)"
          @previous="focusQuestionByOffset(-1)"
        />
        <Composer
          v-else
          v-model="prompt"
          :max-width="CONVERSATION_COMPOSER_MAX_WIDTH"
          :provider-id="providerId"
          :model-id="modelId"
          :provider-models="providerModels"
          :workspace-name="workspace?.name ?? t('chat.noWorkspace')"
          :workspace-id="workspace?.id ?? null"
          :streaming="agentWorking"
          :disabled="workspace === null"
          :can-send="canSend"
          :can-queue="canQueue"
          :queued-count="queuedForSession"
          :model-disabled="savingModel || enabledModelCount === 0"
          :can-stop="currentSession !== null && !stopping"
          :images="images"
          :image-input-unsupported="imageInputUnsupported"
          :pdfs="pdfs"
          :intents="intents"
          :pdf-unreadable="pdfUnreadable"
          :pdf-reading="pdfReading > 0"
          :web-search-enabled="webSearchEnabled"
          :tool-endpoint-ready="toolEndpointReady"
          :browser-enabled="browserToolEnabled"
          @select-model="handleSelectModel"
          @send="handleSend"
          @queue="handleQueue"
          @stop="handleStop"
          @pick-images="handlePickImages"
          @remove-image="handleRemoveImage"
          @paste-images="handlePasteImages"
          @drop-files="handleDropFiles"
          @pick-pdf="handlePickPdf"
          @remove-pdf="handleRemovePdf"
          @toggle-intent="handleToggleIntent"
        />
        <p class="mt-2 text-center text-xs text-muted-foreground">
          {{ t("chat.turnsAndSteps", { turns, steps }) }}
        </p>
      </div>
    </template>
  </div>
</template>
