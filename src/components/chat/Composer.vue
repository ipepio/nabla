<script setup lang="ts">
// Composer card (spec S2): auto-growing prompt, inert "+" and scope chip, the
// provider → model picker and the circular send/stop control.
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
import { computed, nextTick, onMounted, ref, watch } from "vue";
import {
  ArrowUpIcon,
  CheckIcon,
  ChevronDownIcon,
  PlusIcon,
  SquareIcon,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
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
  }>(),
  { maxWidth: COMPOSER_MAX_WIDTH },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: string): void;
  (e: "selectModel", choice: ModelChoice): void;
  (e: "send"): void;
  (e: "queue"): void;
  (e: "stop"): void;
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
  if (props.modelValue.trim() === "") return false;
  return props.streaming ? props.canQueue : props.canSend;
});

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
      `composer submit: nothing to do (text=${props.modelValue.trim() !== ""} streaming=${props.streaming} canQueue=${props.canQueue} canSend=${props.canSend} disabled=${props.disabled})`,
    );
    return;
  }
  // The draft is leaving the composer: a list built from it has nothing left to
  // complete.
  closeCompletion();
  debugLog(`composer submit: ${props.streaming ? "queue" : "send"}`);
  if (props.streaming) emit("queue");
  else emit("send");
}

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
    />

    <div class="flex min-w-0 items-center gap-1 px-2 pb-2">
      <!-- Seam: attaching files/context is not wired yet. -->
      <Button
        variant="ghost"
        size="icon-sm"
        :aria-label="t('chat.composer.addContext')"
        :disabled="disabled"
      >
        <PlusIcon />
      </Button>

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
