<script setup lang="ts">
// Composer card (spec S2): auto-growing prompt, the image button, the scope chip,
// the provider → model picker and the circular send/stop control.
//
// Presentation only: every action is emitted, so ChatPanel keeps the single
// `sendPrompt` / `stopSession` path and this card never touches the contracts
// seam. Colours come from semantic tokens only (S4) — no hex here.
//
// It also owns the two autocompletes of `odd/tasks/composer-inputs.md` (`/`
// commands and `@` workspace paths) and hosts the effort selector. The rules
// themselves live in `composer-completion.ts` (what is a trigger, what a pick
// inserts) and in the two small components this card renders, so the wiring here
// is only "what did the textarea just ask for, and what does that pick mean".
//
// S1-S2 of `odd/tasks/composer-image-input.md`: the `+` button, a screenshot
// paste and a desktop drop all ask the panel to attach an image; the previews and
// their remove controls are drawn here, from the list the panel owns. What counts
// as an image, how many fit and what a clipboard paste means are rules of
// `composer-images.ts`, never restated in this template.
//
// S2-S3 of `odd/tasks/composer-tools-pdf.md`: the same `+` is now a menu with the
// two attachments (an image, a PDF) and the per-turn tool requests. The request
// rows are rules of `composer-tools.ts` — offered only when this run really has
// the capability, and otherwise disabled with the reason in words — and the PDF
// chips are rules of `composer-pdfs.ts`. Neither selection executes anything: the
// request becomes one line of the turn's text, and the page-numbered document is
// composed by `composer-turn.ts` when the panel sends or queues.
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import {
  ArrowUpIcon,
  CheckIcon,
  ChevronDownIcon,
  FileTextIcon,
  ImageIcon,
  PlusIcon,
  SquareIcon,
  WrenchIcon,
  XIcon,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Textarea } from "@/components/ui/textarea";
import { t } from "@/lib/i18n";
import { COMPOSER_MAX_WIDTH, COMPOSER_MIN_WIDTH } from "@/lib/layout";
import type { FsEntry, ProviderModels, SessionCommand } from "@/contracts/types";
import {
  debugLog,
  fsListDir,
  listWorkspaces,
  sessionCommands,
  workspaceCommands,
} from "@/contracts/commands";
import {
  applySuggestion,
  findCompletionTrigger,
  splitPathQuery,
  type CompletionTrigger,
  type SuggestionItem,
} from "@/components/chat/composer-completion";
import ComposerEffort from "@/components/chat/ComposerEffort.vue";
import ComposerSuggestions from "@/components/chat/ComposerSuggestions.vue";
import {
  isInsideDropTarget,
  shouldAttachFromClipboard,
  type ComposerImage,
} from "@/components/chat/composer-images";
import type { ComposerPdf } from "@/components/chat/composer-pdfs";
import {
  COMPOSER_TOOL_INTENTS,
  toolIntentLabelKey,
  toolUnavailableReason,
  toolUnavailableReasonKey,
  type ComposerToolIntent,
} from "@/components/chat/composer-tools";
import { onFileDrop } from "@/contracts/window";
import { activeSession } from "@/lib/session-store";
import PermissionModeChip from "@/components/chat/PermissionModeChip.vue";

/**
 * A model choice is provider-scoped (S2 of `models-by-provider.md`): the same id
 * can exist under two providers, so picking one fixes both halves at once.
 */
interface ModelChoice {
  provider: string;
  model: string;
}

/**
 * One row of the picker: a provider and the models it *can* offer. The label is
 * pi's own provider name and falls back to the id when it is empty.
 */
interface ProviderOption {
  id: string;
  name: string;
  models: ProviderModels["models"];
}

const props = withDefaults(
  defineProps<{
    /** Prompt text (v-model). */
    modelValue: string;
    /**
     * Providers with their models, from `list_provider_models` (S1 of
     * `providers-pane-refinements.md`). The UI never hardcodes a model id: the
     * picker paints exactly what pi's file holds.
     */
    providerModels: ProviderModels[];
    /** Persisted provider id from settings.json, `""` until settings load. */
    providerId: string;
    /** Persisted model id from settings.json, `""` until settings load. */
    modelId: string;
    /** Workspace the turn is written into, for the scope chip. */
    workspaceName: string;
    /**
     * The workspace on screen, `null` before one is chosen. Both autocompletes
     * need it and neither needs a session: `@` lists the paths of this directory,
     * and `/` probes pi for this workspace's commands when no session is live.
     */
    workspaceId: number | null;
    /**
     * The agent is working (a send is in flight or the turn is streaming): the
     * primary control becomes stop, and submitting queues while S3's queue is on.
     */
    streaming: boolean;
    /** Nothing can be sent yet (no workspace selected). */
    disabled: boolean;
    /** The prompt is non-empty and no send is in flight. */
    canSend: boolean;
    /**
     * S3 of `chat-preferences-and-queue.md`: the agent is working, `queueMessages`
     * is on and the prompt is non-empty — so submitting queues instead of
     * blocking. Computed by the panel, which owns the preference and the session
     * state; the card only decides what the click means.
     */
    canQueue: boolean;
    /** How many prompts are waiting in this session's queue (S3). */
    queuedCount: number;
    /**
     * Provider/model selection is unavailable (saving, or no enabled model
     * reported): the single trigger is disabled, so no half choice is possible.
     */
    modelDisabled: boolean;
    /** A session exists, so stop is meaningful. */
    canStop: boolean;
    /**
     * Ceiling for the card, chosen by the caller from `src/lib/layout.ts`: the
     * empty state keeps `COMPOSER_MAX_WIDTH`, the conversation state may use the
     * wider `CONVERSATION_COMPOSER_MAX_WIDTH`. Only the ceiling moves — the floor
     * is always `COMPOSER_MIN_WIDTH`, because it is the chat section's minimum.
     */
    maxWidth?: number;
    /**
     * S1-S2 of `odd/tasks/composer-image-input.md`: the images of the draft on
     * screen, oldest first. The panel owns them (they belong to the draft key, see
     * `src/lib/composer-attachments.ts`); the card paints and removes them.
     */
    images: ComposerImage[];
    /**
     * S2: the active model reports no image input, so this turn would be refused
     * before it left. The notice explains it and submit stays inert — never a
     * silent model switch, which is what this flag exists to prevent.
     */
    imageInputUnsupported: boolean;
    /** S3: the PDFs of the draft on screen, in attachment order. */
    pdfs: ComposerPdf[];
    /** S2: the tool requests chosen for this draft, in canonical order. */
    intents: ComposerToolIntent[];
    /**
     * S3: at least one attached PDF has no selectable text (a scan). It cannot be
     * sent, and the notice says why; the chip's remove control is the way out.
     */
    pdfUnreadable: boolean;
    /** S3: an extraction is running, so the composer says it is reading. */
    pdfReading: boolean;
    /** S2: `settings.webSearchEnabled` — the human turned the `web_search` tool on. */
    webSearchEnabled: boolean;
    /**
     * S2: this run bound the MCP endpoint the agent's tools are published from
     * (`web_search` and the browser's). `false` means the agent has neither, so
     * both request rows are disabled with the reason instead of being offered as
     * if they worked.
     */
    toolEndpointReady: boolean;
    /**
     * S2: the Browser tool is on in Settings → Tools (`enabledTools`). The browser
     * request needs it: the switch ships off because the browser drives a real
     * Chromium instance, so a row that ignored it would advertise a tool the human
     * never turned on.
     */
    browserEnabled: boolean;
  }>(),
  { maxWidth: COMPOSER_MAX_WIDTH },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: string): void;
  (e: "selectModel", choice: ModelChoice): void;
  (e: "send"): void;
  (e: "queue"): void;
  (e: "stop"): void;
  (e: "pickImages"): void;
  (e: "removeImage", id: string): void;
  (e: "pasteImages", files: File[]): void;
  (e: "dropFiles", paths: string[]): void;
  (e: "pickPdf"): void;
  (e: "removePdf", id: string): void;
  (e: "toggleIntent", intent: ComposerToolIntent): void;
}>();

/**
 * S3: while the agent works and the queue is on, submitting enqueues the prompt
 * for this session instead of sending it into a busy process. The panel keeps the
 * preference and the queue; this is only which of the two events the click (or
 * Enter) produces.
 */
const queueing = computed(() => props.streaming && props.canQueue);

/**
 * Whether a submit has somewhere to go: a send while the agent is idle, a queued
 * prompt while it works. One function, read by the button's disabled state AND by
 * the handler below, so the two can never disagree about what a click means —
 * which is what a click that reached an enabled-looking control and returned
 * silently used to look like.
 *
 * The empty-draft and no-workspace checks live here rather than being inherited
 * from `canSend`/`canQueue`: the same list of reasons the control is inert now
 * answers a click and paints the button.
 */
const canSubmit = computed(() => {
  if (props.disabled) return false;
  // S2 of `composer-image-input.md`: an image-only turn is a turn. The typed text
  // is not the only thing that can make a prompt worth sending.
  if (
    props.modelValue.trim() === "" &&
    props.images.length === 0 &&
    props.pdfs.length === 0
  ) {
    return false;
  }
  // S3 of `composer-tools-pdf.md`: a scan carries no text, so it is never sent.
  if (props.imageInputUnsupported || props.pdfUnreadable) return false;
  return props.streaming ? props.canQueue : props.canSend;
});

/**
 * S2: the menu rows for the tool requests, with the availability answer each one
 * gets from the pure rules. A row that cannot be offered keeps its place — the
 * human sees what exists and what would make it usable — and is disabled. Both
 * switches are facts of this run (the Browser tool in Settings → Tools, the web
 * search preference); the unproven SearXNG round trip is the module's own fact, so
 * `web-search` stays disabled here even with everything the panel can see ready.
 */
const toolRows = computed(() => {
  const context = {
    webSearchEnabled: props.webSearchEnabled,
    toolEndpointReady: props.toolEndpointReady,
    browserEnabled: props.browserEnabled,
  };
  return COMPOSER_TOOL_INTENTS.map((intent) => {
    const reason = toolUnavailableReason(intent, context);
    return {
      intent,
      label: t(toolIntentLabelKey(intent)),
      available: reason === null,
      reason: reason === null ? "" : t(toolUnavailableReasonKey(reason)),
    };
  });
});

/** Whether anything (besides the text) is attached to this draft. */
const hasAttachments = computed(
  () =>
    props.images.length > 0 ||
    props.pdfs.length > 0 ||
    props.intents.length > 0,
);

/** S2: one row of the menu is selectable only when the capability exists. */
function intentAvailable(intent: ComposerToolIntent): boolean {
  // Read from the rows above so the disabled state and the reason can never
  // disagree about the same request.
  return toolRows.value.find((row) => row.intent === intent)?.available ?? false;
}

/**
 * What one submit means. The button and the Enter key share this, so they can
 * never disagree, and it is the only place that decides between sending and
 * queueing.
 *
 * A submit that has nowhere to go is logged instead of vanishing: "my click did
 * nothing" is the report this answers, and the dev terminal is where it can be
 * answered (structure only — never the prompt).
 */
function submit(): void {
  if (!canSubmit.value) {
    debugLog(
      `composer submit: nothing to do (text=${props.modelValue.trim() !== ""} images=${props.images.length} pdfs=${props.pdfs.length} streaming=${props.streaming} canQueue=${props.canQueue} canSend=${props.canSend} disabled=${props.disabled})`,
    );
    return;
  }
  // The draft is leaving the composer: a list built from it has nothing left to
  // complete.
  closeCompletion();
  debugLog(
    `composer submit: ${props.streaming ? "queue" : "send"} (images=${props.images.length} pdfs=${props.pdfs.length} tools=${props.intents.length})`,
  );
  if (props.streaming) emit("queue");
  else emit("send");
}

// ── image input (S1-S2 of `composer-image-input.md`) ───────────────────────

/** The card, so a native drop can be told whether it landed on the composer. */
const card = ref<HTMLElement | null>(null);

/**
 * S1: a screenshot paste. `shouldAttachFromClipboard` owns the rule, which is what
 * keeps pasting text untouched: only an image-only clipboard is an attachment,
 * and only then is the browser's own paste cancelled.
 */
function handlePaste(event: ClipboardEvent): void {
  if (props.disabled) return;
  const data = event.clipboardData;
  if (data === null) return;
  if (!shouldAttachFromClipboard({ items: data.items, text: data.getData("text/plain") })) {
    return;
  }
  const files: File[] = [];
  for (const item of data.items) {
    if (item.kind !== "file") continue;
    const file = item.getAsFile();
    if (file !== null) files.push(file);
  }
  if (files.length === 0) return;
  event.preventDefault();
  emit("pasteImages", files);
}

/**
 * S1: a desktop drop. On Tauri the OS delivers the paths and the position to the
 * window instead of the DOM, so the subscription is window-wide and this decides
 * whether it landed here (`isInsideDropTarget`, physical pixels against the
 * card's CSS rectangle).
 */
let stopDropListener: (() => void) | null = null;
/** Set when the card goes away while the subscription is still being installed. */
let dropListenerDisposed = false;

onMounted(async () => {
  const stop = await onFileDrop((event) => {
    if (props.disabled || event.paths.length === 0) return;
    const element = card.value;
    if (element === null) return;
    if (
      event.position !== null &&
      !isInsideDropTarget(
        event.position,
        element.getBoundingClientRect(),
        window.devicePixelRatio,
      )
    ) {
      return;
    }
    emit("dropFiles", event.paths);
  });
  // A card that unmounted while the subscription was resolving keeps nothing.
  if (dropListenerDisposed) {
    stop();
    return;
  }
  stopDropListener = stop;
});

onUnmounted(() => {
  dropListenerDisposed = true;
  stopDropListener?.();
  stopDropListener = null;
});

// ── autocompletes (/ commands, @ workspace paths) ──────────────────────────
//
// The trigger rule and the "what a pick inserts" rule live in
// `composer-completion.ts`; this is the wiring. Every entry point is an event on
// the textarea (input, keydown, mousedown on a row), so no watcher is involved
// and nothing runs while the human is not typing.

/** The open autocomplete, or `null` when the list is closed. */
const trigger = ref<CompletionTrigger | null>(null);
/** The rows of the open list, already filtered and ordered. */
const items = ref<SuggestionItem[]>([]);
/** The highlighted row; the arrows move it, Enter and a click take it. */
const activeIndex = ref(0);
/**
 * The live textarea, captured from its own events. Completing a suggestion has to
 * move the caret, and the element that owns the caret is the one that just told
 * us what was typed — no template ref and no DOM query needed.
 */
const textareaEl = ref<HTMLTextAreaElement | null>(null);
/**
 * The session's commands, keyed by session id: a list that only changes when pi
 * starts a session is worth one read per session, not one per keystroke. A
 * failed read is deliberately NOT cached, so a session that was still spawning
 * when the first `/` was typed answers on the next attempt.
 */
const commandsBySession = new Map<string, SessionCommand[]>();
/**
 * The workspace's commands as this app run last saw them, from the on-demand
 * probe (`workspace_commands`).
 *
 * In memory only, and deliberately so: a list kept across launches is a
 * snapshot, and a snapshot is what the probe replaced. Cached for the run, keyed
 * by workspace, because the probe costs a child's whole boot and a keystroke
 * must never pay it twice. The first `/` of a launch waits for that boot (pi
 * takes a second or two); after it, the list is instant. An empty answer is
 * cached too — the alternative is a fresh child per keystroke against a pi that
 * just said it has nothing.
 */
const probedByWorkspace = new Map<number, SessionCommand[]>();
/**
 * The probe in flight, per workspace. Rust already shares one child between
 * concurrent callers; this just keeps a burst of keystrokes from stacking a
 * dozen IPC calls onto the same wait.
 */
const probingWorkspaces = new Map<number, Promise<SessionCommand[]>>();
/** The workspace path of the session, resolved once for the `@` walk. */
let workspacePath: { id: number; path: string } | null = null;
/** Bumped per request, so a slow answer cannot overwrite a newer list. */
let completionRequest = 0;

/** The list is only open while it has something to show. */
const suggestionsOpen = computed(
  () => trigger.value !== null && items.value.length > 0,
);

/** What the open list is announcing: the commands of the session, or the workspace. */
const suggestionsTitle = computed(() =>
  trigger.value?.kind === "command"
    ? t("chat.composer.commands")
    : t("chat.composer.workspacePaths"),
);

function closeCompletion(): void {
  trigger.value = null;
  items.value = [];
  activeIndex.value = 0;
  // Any answer still in flight belongs to the list that just closed.
  completionRequest += 1;
}

/**
 * The textarea's own input event. The value it carries is the one on screen right
 * now, which is what the caret arithmetic needs: `modelValue` is a render behind
 * during the same tick.
 */
function handleInput(event: Event): void {
  const element = event.target;
  if (!(element instanceof HTMLTextAreaElement)) return;
  textareaEl.value = element;
  void refreshCompletion(element.value, element.selectionStart ?? element.value.length);
}

/**
 * Opens, re-filters or closes the list for one caret position.
 *
 * `kind` is decided by the text, never by which feature opened last: `/fix` asks
 * for commands, `@src/a` for paths, and anything else closes the list.
 */
async function refreshCompletion(text: string, caret: number): Promise<void> {
  const found = findCompletionTrigger(text, caret);
  // The workspace is what both lists need. A session only makes the command list
  // *fresh*: paths come from the directory, and with no session the commands come
  // from a short-lived probe of this workspace. Gating on a session kept both
  // shut until a prompt had already been sent — exactly when they are least
  // useful.
  const workspace = activeSession.value?.workspaceId ?? props.workspaceId;
  if (workspace === null || found === null) {
    closeCompletion();
    return;
  }
  trigger.value = found;
  const request = ++completionRequest;
  const rows =
    found.kind === "command"
      ? filterCommands(await commandsFor(workspace), found.query)
      : await pathSuggestions(workspace, found.query);
  // A newer keystroke already asked for its own list.
  if (request !== completionRequest) return;
  items.value = rows;
  activeIndex.value = 0;
}

/**
 * S3: the commands of one workspace. A live session is asked directly; with no
 * session the app runs a short-lived probe (`workspace_commands`). An answer that
 * is not available (pi still booting, no pi installed, the bridge down) is
 * "nothing to offer yet", never an error: the list simply does not open, and the
 * next `/` asks again.
 */
async function commandsFor(workspaceId: number): Promise<SessionCommand[]> {
  const sessionId = activeSession.value?.sessionId ?? null;
  if (sessionId !== null) {
    const cached = commandsBySession.get(sessionId);
    if (cached !== undefined) return cached;
    try {
      const commands = await sessionCommands(workspaceId, Number(sessionId));
      commandsBySession.set(sessionId, commands);
      return commands;
    } catch {
      // A session that cannot answer yet is not a reason to close the list. What
      // the app already knows about this workspace is offered instead — and no
      // second child is spawned while a session is live.
      return probedByWorkspace.get(workspaceId) ?? [];
    }
  }
  return probedCommands(workspaceId);
}

/**
 * The commands of a workspace with no live session, from the on-demand probe.
 *
 * The probe spawns pi, asks `get_commands` and kills it, so the list is current
 * instead of remembered; Rust answers an empty list when it cannot ask at all,
 * and a rejected call degrades to the same empty list here. Nothing is written
 * to storage: leaving the app is enough to forget the snapshot.
 */
async function probedCommands(workspaceId: number): Promise<SessionCommand[]> {
  const known = probedByWorkspace.get(workspaceId);
  if (known !== undefined) return known;
  const running = probingWorkspaces.get(workspaceId);
  if (running !== undefined) return running;
  const probe = workspaceCommands(workspaceId)
    .catch(() => {
      // A bridge that cannot be reached is "no list", never a dialog: the
      // human is typing, and `/` opening nothing is the honest answer.
      return [] as SessionCommand[];
    })
    .then((commands) => {
      probedByWorkspace.set(workspaceId, commands);
      return commands;
    })
    .finally(() => {
      probingWorkspaces.delete(workspaceId);
    });
  probingWorkspaces.set(workspaceId, probe);
  return probe;
}

/** The source badge of a command: pi's own word, translated when this build knows it. */
function sourceBadge(source: string): string {
  switch (source) {
    case "extension":
      return t("chat.composer.commandSource.extension");
    case "prompt":
      return t("chat.composer.commandSource.prompt");
    case "skill":
      return t("chat.composer.commandSource.skill");
    default:
      // A source from a newer pi: show the word rather than a badge that lies.
      return source;
  }
}

/**
 * S3: the commands whose name starts with what was typed, with the slash and a
 * trailing space as the insertion — sending that text is what runs the command,
 * through the existing prompt path (pi expands it). Capped so a session with a
 * very long skill list stays one glance, and never truncated silently: the human
 * narrows with two more characters, which is faster than scrolling.
 */
function filterCommands(
  commands: SessionCommand[],
  query: string,
): SuggestionItem[] {
  const needle = query.trim().toLowerCase();
  const limit = 30;
  const rows: SuggestionItem[] = [];
  for (const command of commands) {
    if (!command.name.toLowerCase().startsWith(needle)) continue;
    rows.push({
      key: command.name,
      label: `/${command.name}`,
      hint: command.description,
      badge: sourceBadge(command.source),
      insert: `/${command.name} `,
    });
    if (rows.length === limit) break;
  }
  return rows;
}

/**
 * S4: the entries of the directory the caret is typing in, one level at a time.
 *
 * The walk is lazy on purpose: only the current segment's directory is read, with
 * the same workspace-confined `fsListDir` the Files tool uses, so a mention can
 * never reach outside the workspace and never costs a tree walk. A directory is
 * inserted with a trailing `/` and keeps the list open on the next segment; a
 * file completes the path and closes it.
 */
async function pathSuggestions(
  workspaceId: number,
  query: string,
): Promise<SuggestionItem[]> {
  const root = await workspaceOf(workspaceId);
  if (root === "") return [];
  const { dir, filter } = splitPathQuery(query);
  let entries: FsEntry[];
  try {
    entries = await fsListDir(root, dir);
  } catch {
    // A segment that does not exist (a typo, or a path that left the workspace)
    // is "nothing to offer", not a dialog to dismiss.
    return [];
  }
  const needle = filter.toLowerCase();
  const prefix = dir === "" ? "" : `${dir}/`;
  return entries
    .filter((entry) => entry.name.toLowerCase().startsWith(needle))
    .sort((left, right) => {
      const leftDir = left.kind === "dir" ? 0 : 1;
      const rightDir = right.kind === "dir" ? 0 : 1;
      return leftDir - rightDir || left.name.localeCompare(right.name);
    })
    .slice(0, 30)
    .map((entry) => {
      const path = `${prefix}${entry.name}`;
      const directory = entry.kind === "dir";
      return {
        key: path,
        label: `@${path}${directory ? "/" : ""}`,
        badge: directory
          ? t("chat.composer.folder")
          : t("chat.composer.file"),
        insert: `@${path}${directory ? "/" : ""}`,
        directory,
      };
    });
}

/**
 * The absolute path of the session's workspace, resolved once and reused: the `@`
 * picker needs it for `fsListDir`, and the composer only knows the workspace's
 * name. A workspace that cannot be resolved offers no paths instead of guessing
 * one.
 */
async function workspaceOf(workspaceId: number): Promise<string> {
  if (workspacePath !== null && workspacePath.id === workspaceId) {
    return workspacePath.path;
  }
  try {
    const workspaces = await listWorkspaces();
    const found = workspaces.find((entry) => entry.id === workspaceId);
    if (found === undefined) return "";
    workspacePath = { id: workspaceId, path: found.path };
    return found.path;
  } catch {
    return "";
  }
}

/**
 * Replaces the trigger token with the picked row and puts the caret after it.
 *
 * The insertion rules are pi's own shapes: `/name ` is what the prompt path runs,
 * and `@relative/path` is text the agent reads with its tools (there are no
 * attachments in the protocol, and Nabla does not invent one).
 */
function complete(index: number): void {
  const open = trigger.value;
  const item = items.value[index];
  if (open === null || item === undefined) return;
  const element = textareaEl.value;
  const text = element?.value ?? props.modelValue;
  const caret = element?.selectionStart ?? text.length;
  const applied = applySuggestion(text, open, caret, item.insert);
  emit("update:modelValue", applied.text);
  void nextTick(() => {
    element?.focus();
    element?.setSelectionRange(applied.caret, applied.caret);
  });
  if (item.directory === true) {
    // The next segment is what a directory pick is for: walk into it.
    void refreshCompletion(applied.text, applied.caret);
  } else {
    closeCompletion();
  }
}

function moveActive(step: number): void {
  const count = items.value.length;
  if (count === 0) return;
  activeIndex.value = (activeIndex.value + step + count) % count;
}

/**
 * Every key of the textarea. Two rules, in this order:
 *
 *   1. while the list is OPEN, the arrows walk it and Enter or Tab completes —
 *      so Enter can never both complete and send;
 *   2. otherwise Enter does exactly what it always did: send the prompt (or queue
 *      it), and Shift+Enter still inserts a newline.
 *
 * A list that is closed (or open with nothing in it — a session with no commands
 * yet) hijacks nothing: its keys fall through to rule 2.
 */
function onKeydown(event: KeyboardEvent): void {
  const element = event.target;
  if (element instanceof HTMLTextAreaElement) textareaEl.value = element;
  if (suggestionsOpen.value) {
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        moveActive(1);
        return;
      case "ArrowUp":
        event.preventDefault();
        moveActive(-1);
        return;
      case "Enter":
      case "Tab":
        event.preventDefault();
        complete(activeIndex.value);
        return;
      case "Escape":
        event.preventDefault();
        closeCompletion();
        return;
      default:
        break;
    }
  }
  if (event.key === "Enter" && !event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey) {
    event.preventDefault();
    submit();
  }
}

/**
 * One dropdown in the footer: every provider is a row that opens that provider's
 * models in a lateral submenu, so a pick is still one interaction (and one
 * settings write) for the provider + model pair. Two marks keep a model out of
 * that list: `enabled === false` is pi's usability mark and `hidden === true` is
 * Nabla's display preference — both are marks, never deletes, so the entry stays
 * in the file either way.
 *
 * A provider keeps its row — disabled when it has nothing usable — as long as the
 * human has not hidden every model of it: a provider whose whole list is hidden is
 * not offered at all.
 */
const providerRows = computed<ProviderOption[]>(() =>
  props.providerModels
    .filter((entry) => entry.models.some((model) => model.hidden !== true))
    .map((entry) => ({
      id: entry.provider,
      name: entry.name.trim() === "" ? entry.provider : entry.name,
      models: entry.models.filter(
        (model) => model.enabled && model.hidden !== true,
      ),
    })),
);

/** Rows that can actually be picked from: the trigger's enabled/fallback set. */
const providerOptions = computed<ProviderOption[]>(() =>
  providerRows.value.filter((option) => option.models.length > 0),
);

/**
 * The pair the session is really using while the picker no longer offers it: the
 * persisted provider and model still resolve in pi's read, but the human hid the
 * model. The trigger keeps naming it — the session keeps using it — and the menu
 * keeps not offering it. View-only: nothing is written until the human picks.
 */
const keptSelection = computed<ProviderOption | null>(() => {
  if (props.modelId === "") return null;
  const entry = props.providerModels.find(
    (candidate) => candidate.provider === props.providerId,
  );
  const model = entry?.models.find((item) => item.id === props.modelId);
  if (!entry || !model || model.hidden !== true) return null;
  return {
    id: entry.provider,
    name: entry.name.trim() === "" ? entry.provider : entry.name,
    models: [model],
  };
});

/**
 * The provider on screen: the persisted one while it still has models, otherwise
 * the provider of a persisted model the human just hid (it is still in use), else
 * the first provider that has anything (S1). That fallback is view-only — settings
 * are written only when the human picks, never on render — and it is also what a
 * provider that just disappeared from pi's read leaves on screen.
 */
const activeProvider = computed<ProviderOption | null>(() => {
  const option = providerOptions.value.find(
    (candidate) => candidate.id === props.providerId,
  );
  return option ?? keptSelection.value ?? providerOptions.value[0] ?? null;
});

/**
 * The model on screen: the persisted one while the provider on screen carries it
 * — including a model the human hid, because the session keeps using that one —
 * and the first usable model of the fallback provider, so a selection whose
 * provider vanished names something real instead of a placeholder. `null` means
 * the trigger shows its placeholder instead of a value from another provider.
 */
const activeModel = computed(() => {
  const provider = activeProvider.value;
  if (provider === null) return null;
  if (provider.id !== props.providerId) return provider.models[0] ?? null;
  if (props.modelId === "") return null;
  return (
    provider.models.find((model) => model.id === props.modelId) ??
    keptSelection.value?.models[0] ??
    null
  );
});

/**
 * A persisted model the provider on screen does not carry (a file edit, a
 * disabled mark) is still named instead of being hidden behind the placeholder,
 * but only while the persisted provider is the one on screen: under the first-
 * provider fallback the stored model belongs elsewhere and naming it would lie.
 */
const unresolvedModelId = computed(() => {
  const provider = activeProvider.value;
  if (provider === null || provider.id !== props.providerId) return "";
  if (props.modelId === "" || activeModel.value !== null) return "";
  return props.modelId;
});

/**
 * What the trigger's model half reads: the model's display name while the pair
 * resolves, the raw persisted id while it does not, `""` when there is nothing
 * to show and the placeholder takes over.
 */
const activeModelLabel = computed(
  () => activeModel.value?.name ?? unresolvedModelId.value,
);

/** Hover/long-press text for the trigger: the full pair, never truncated. */
const triggerLabel = computed(() => {
  const provider = activeProvider.value;
  if (provider === null) return t("chat.composer.selectProvider");
  const model = activeModelLabel.value;
  return model === ""
    ? `${provider.name} · ${t("chat.composer.selectModel")}`
    : `${provider.name} · ${model}`;
});

/** The check mark follows the *persisted* pair, not the fallback on screen. */
function isSelectedPair(provider: string, model: string): boolean {
  return provider === props.providerId && model === props.modelId;
}

/**
 * Picking a model fixes both halves at once (model ids are provider-scoped), so
 * the panel saves the pair with one write; a pick that is already stored writes
 * nothing.
 */
function chooseModel(provider: string, model: string) {
  if (provider === "" || model === "") return;
  if (provider === props.providerId && model === props.modelId) return;
  emit("selectModel", { provider, model });
}
/**
 * Warm the command list before anyone asks for it.
 *
 * The probe costs a couple of seconds and one throwaway pi child. Paying that
 * while the panel is appearing is invisible; paying it on the first `/` is a
 * visible wait, which is exactly what the maintainer felt. One warm-up per
 * workspace per app run — the answer is cached in memory, nothing is persisted,
 * and a live session still refreshes it when it can.
 */
function prewarmCommands(): void {
  if (activeSession.value !== null) return;
  const workspace = props.workspaceId;
  if (workspace === null) return;
  void commandsFor(workspace).catch(() => {});
}

onMounted(prewarmCommands);
// A workspace switch is a new command list: warm that one too, before any caret
// lands in the composer. Reading `props` here is safe — it is declared above.
watch(() => props.workspaceId, prewarmCommands);

</script>

<template>
  <!-- The card is centred and bounded by the caller: the empty state passes the
       narrow ceiling, the conversation state the wide one. -->
  <!-- Bounds come from `src/lib/layout.ts`: the box shrinks with its column and
       stops at half of its normal width, which is also the chat's floor. -->
  <!-- `relative` is the anchor of the autocomplete list: it opens ABOVE the card
       and out of the layout flow, so typing `/` or `@` never moves the text. -->
  <div
    ref="card"
    class="relative mx-auto w-full min-w-0 rounded-2xl border border-border bg-card/60 shadow-sm"
    :style="{ maxWidth: `${maxWidth}px`, minWidth: `${COMPOSER_MIN_WIDTH}px` }"
  >
    <!-- S3/S4: the commands of this session, or the entries of the workspace
         directory the `@` mention is typing in. -->
    <ComposerSuggestions
      v-if="suggestionsOpen"
      :title="suggestionsTitle"
      :items="items"
      :active-index="activeIndex"
      @select="complete"
      @highlight="activeIndex = $event"
    />

    <!-- S1-S3: everything attached to this draft, before the text it travels
         with. An image preview is the same base64 the model will receive; a PDF
         chip names its pages without carrying their text; a tool chip names the
         request the turn will include. Removing any of them only forgets it,
         which is the undo the human asked for. -->
    <div
      v-if="hasAttachments"
      class="flex flex-wrap items-start gap-1.5 px-3 pt-2"
    >
      <div
        v-if="images.length > 0"
        class="flex flex-wrap items-start gap-1.5"
        :aria-label="t('chat.composer.attachedImages')"
      >
        <div
          v-for="image in images"
          :key="image.id"
          class="relative size-14 overflow-hidden rounded-md border border-border"
        >
          <img
            :src="`data:${image.mimeType};base64,${image.data}`"
            :alt="image.name"
            class="size-full object-cover"
          />
          <button
            type="button"
            class="absolute top-0.5 right-0.5 rounded-full bg-background/80 p-0.5 text-foreground shadow-sm hover:bg-background"
            :aria-label="t('chat.composer.removeImage', { name: image.name })"
            @click="emit('removeImage', image.id)"
          >
            <XIcon class="size-3" />
          </button>
        </div>
      </div>

      <!-- S3: one chip per PDF. The page count is what the turn will carry; the
           text itself never enters the UI, so a long report costs one chip. -->
      <div
        v-if="pdfs.length > 0"
        class="flex flex-wrap items-center gap-1.5"
        :aria-label="t('chat.composer.attachedPdfs')"
      >
        <span
          v-for="pdf in pdfs"
          :key="pdf.id"
          class="inline-flex max-w-full items-center gap-1 rounded-md border border-border bg-muted/40 px-1.5 py-1 text-xs"
        >
          <FileTextIcon class="size-3 shrink-0" />
          <span class="min-w-0 max-w-[12rem] truncate" :title="pdf.name">
            {{ pdf.name }}
          </span>
          <span class="shrink-0 text-muted-foreground">
            {{ t("chat.composer.pdfPageCount", { count: pdf.pages.length }) }}
          </span>
          <button
            type="button"
            class="rounded-full p-0.5 text-foreground hover:bg-background"
            :aria-label="t('chat.composer.removePdf', { name: pdf.name })"
            @click="emit('removePdf', pdf.id)"
          >
            <XIcon class="size-3" />
          </button>
        </span>
      </div>

      <!-- S2: the requested tools of this turn, removable like any attachment. -->
      <div
        v-if="intents.length > 0"
        class="flex flex-wrap items-center gap-1.5"
        :aria-label="t('chat.composer.toolRequests')"
      >
        <span
          v-for="intent in intents"
          :key="intent"
          class="inline-flex items-center gap-1 rounded-md border border-border bg-muted/40 px-1.5 py-1 text-xs"
        >
          <WrenchIcon class="size-3 shrink-0" />
          <span>{{ t(toolIntentLabelKey(intent)) }}</span>
          <button
            type="button"
            class="rounded-full p-0.5 text-foreground hover:bg-background"
            :aria-label="
              t('chat.composer.removeTool', {
                name: t(toolIntentLabelKey(intent)),
              })
            "
            @click="emit('toggleIntent', intent)"
          >
            <XIcon class="size-3" />
          </button>
        </span>
      </div>
    </div>

    <!-- S3: a document with no selectable text cannot be sent, and the reason is
         never a guess: no OCR runs here, so its pages would carry nothing. -->
    <p
      v-if="pdfUnreadable"
      class="px-3.5 pt-1.5 text-xs text-warning"
      role="status"
    >
      {{ t("chat.composer.pdfNoText") }}
    </p>
    <p
      v-if="pdfReading"
      class="px-3.5 pt-1.5 text-xs text-muted-foreground"
      role="status"
    >
      {{ t("chat.composer.pdfExtracting") }}
    </p>

    <!-- S2: the explanation that replaces a silent model switch. It is a notice,
         not a dialog: the images and the text stay exactly where the human left
         them. -->
    <p
      v-if="imageInputUnsupported"
      class="px-3.5 pt-1.5 text-xs text-warning"
      role="status"
    >
      {{ t("chat.composer.imageUnsupported") }}
    </p>

    <Textarea
      :model-value="modelValue"
      :aria-label="t('chat.composer.prompt')"
      :placeholder="t('chat.composer.placeholder')"
      class="max-h-64 min-h-[3.7rem] resize-none border-0 bg-transparent px-3.5 pt-2 pb-1.5 text-sm shadow-none focus-visible:ring-0 dark:bg-transparent"
      :disabled="disabled"
      @update:model-value="emit('update:modelValue', String($event))"
      @input="handleInput"
      @keydown="onKeydown"
      @click="handleInput"
      @paste="handlePaste"
    />

    <div class="flex min-w-0 items-center gap-1 px-2 pb-2">
      <!-- S1/S2/S3: attaching (an image or a PDF) and the per-turn tool
           requests. The attachment rows come first and the tool requests under
           their own label; a request that cannot be offered keeps its row,
           disabled, with the reason in words. Selecting one writes no setting and
           runs nothing — it only becomes a line of this turn's text. -->
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button
            variant="ghost"
            size="icon-sm"
            :aria-label="t('chat.composer.attach')"
            :disabled="disabled"
          >
            <PlusIcon />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" side="top" class="w-72">
          <DropdownMenuItem class="gap-2 text-xs" @select="emit('pickImages')">
            <ImageIcon class="size-3.5 shrink-0" />
            {{ t("chat.composer.attachImage") }}
          </DropdownMenuItem>
          <DropdownMenuItem class="gap-2 text-xs" @select="emit('pickPdf')">
            <FileTextIcon class="size-3.5 shrink-0" />
            {{ t("chat.composer.attachPdf") }}
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuLabel class="text-xs">
            {{ t("chat.composer.useTool") }}
          </DropdownMenuLabel>
          <DropdownMenuCheckboxItem
            v-for="row in toolRows"
            :key="row.intent"
            class="items-start text-xs"
            :model-value="intents.includes(row.intent)"
            :disabled="!intentAvailable(row.intent)"
            @update:model-value="emit('toggleIntent', row.intent)"
          >
            <span class="flex min-w-0 flex-col gap-0.5">
              <span class="truncate">{{ row.label }}</span>
              <!-- A disabled row always says why, so "nothing happens" is never
                   the whole answer. -->
              <span
                v-if="!row.available"
                class="text-[10px] leading-snug text-muted-foreground"
              >
                {{ row.reason }}
              </span>
            </span>
          </DropdownMenuCheckboxItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <span
        class="hidden min-w-0 items-center gap-1 rounded-md px-1.5 py-1 text-xs text-muted-foreground sm:inline-flex"
      >
        <span class="truncate">
          {{ t("chat.composer.writingIn", { workspace: workspaceName }) }}
        </span>
        <ChevronDownIcon class="size-3 shrink-0" />
      </span>

      <!-- S3: how many prompts of this session are waiting for the current
           turn to finish. Muted, not a control: the queue drains by itself. -->
      <span
        v-if="queuedCount > 0"
        class="shrink-0 px-1.5 text-xs text-muted-foreground"
      >
        {{ t("chat.composer.queued", { count: queuedCount }) }}
      </span>

      <div class="ms-auto flex min-w-0 shrink items-center gap-1.5">
        <!-- Permission mode: what the agent is allowed to do right now, next to
             the controls that shape the turn it will run in. It belongs with the
             composer — the row where the human decides what happens next — not
             in the header, which only names the conversation. -->
        <PermissionModeChip />

        <!-- S2 of `composer-inputs.md`: the effort of the session, beside the
             permission chip — the two controls that shape the next turn. It
             renders the level pi CONFIRMED, not the one the UI asked for. -->
        <ComposerEffort />

        <!-- Provider → model: one trigger naming the current pair (provider as
             a muted prefix), and one menu where each provider row opens its own
             enabled models laterally. Reka owns the keyboard walk: no custom
             click handler on the sub trigger, so arrows/hover still open it. -->
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button
              variant="ghost"
              size="sm"
              class="h-8 max-w-[220px] min-w-0 gap-1 border-0 bg-transparent px-2 text-xs font-normal shadow-none hover:bg-muted aria-expanded:bg-muted sm:max-w-[280px] dark:bg-transparent dark:hover:bg-muted"
              :disabled="modelDisabled || providerOptions.length === 0"
              :aria-label="t('chat.composer.model')"
              :title="triggerLabel"
            >
              <template v-if="activeProvider !== null">
                <span class="shrink-0 text-muted-foreground">
                  {{ activeProvider.name }}
                </span>
                <!-- A model the provider does not carry is still named instead
                     of being hidden behind the placeholder. -->
                <span
                  v-if="activeModelLabel !== ''"
                  class="min-w-0 truncate"
                >
                  {{ activeModelLabel }}
                </span>
                <span v-else class="min-w-0 truncate text-muted-foreground">
                  {{ t("chat.composer.selectModel") }}
                </span>
              </template>
              <span v-else class="min-w-0 truncate text-muted-foreground">
                {{ t("chat.composer.selectProvider") }}
              </span>
              <ChevronDownIcon class="size-3 shrink-0 opacity-60" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            side="top"
            class="w-56"
            :aria-label="t('chat.composer.provider')"
          >
            <DropdownMenuSub
              v-for="row in providerRows"
              :key="row.id"
            >
              <!-- Disabled, not removed: a provider with nothing usable keeps its
                   row but cannot be opened. A provider whose every model is
                   hidden is not listed at all. -->
              <DropdownMenuSubTrigger
                :disabled="row.models.length === 0"
                class="text-xs data-[disabled]:opacity-50"
              >
                <span class="min-w-0 truncate">{{ row.name }}</span>
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent class="w-56 max-h-[280px] overflow-y-auto">
                <DropdownMenuItem
                  v-for="model in row.models"
                  :key="model.id"
                  class="text-xs"
                  @select="chooseModel(row.id, model.id)"
                >
                  <CheckIcon
                    v-if="isSelectedPair(row.id, model.id)"
                    class="size-3.5 shrink-0"
                  />
                  <span v-else class="size-3.5 shrink-0" aria-hidden="true" />
                  <span class="min-w-0 truncate">{{ model.name }}</span>
                </DropdownMenuItem>
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          </DropdownMenuContent>
        </DropdownMenu>

        <!-- S3: stop and submit sit side by side while the agent works, so a
             prompt can still be queued for the turn after the current one.
             With the queue off, submit is disabled while working: the old
             behaviour, unchanged.

             `mousedown.prevent` keeps the focus in the composer and keeps the
             press and the release on the same element: a click whose two halves
             land on different nodes never fires, and the row under the pointer
             is the one that gains a Stop button and a queue strip at the edge of
             a turn. Preventing the default here does not cancel the click. -->
        <Button
          v-if="streaming && canStop"
          variant="outline"
          size="icon-sm"
          class="rounded-full"
          :aria-label="t('chat.composer.stop')"
          @click="emit('stop')"
        >
          <SquareIcon />
        </Button>
        <Button
          size="icon-sm"
          class="rounded-full"
          :aria-label="
            queueing ? t('chat.composer.queue') : t('chat.composer.send')
          "
          :disabled="!canSubmit"
          @mousedown.prevent
          @click="submit"
        >
          <ArrowUpIcon />
        </Button>
      </div>
    </div>
  </div>
</template>
