<script setup lang="ts">
// The read-only view of one subagent (S3 of `odd/tasks/subagent-helpers.md`: the
// session "se muestra en vivo y en solo lectura", and the command path stays in the
// parent chat).
//
// This is a *state* of the chat panel, not a page: the parent chat was never left,
// only covered, so going back is a reset of one ref and nothing is stopped,
// restarted or re-read (S4 of `session-lifecycle.md`).
//
// The rendering is the conversation's own machinery — `deriveTurnGroups` plus
// `TurnGroup` — fed with the child's pi JSONL through `sessionTranscript`. There is
// no composer, no queue, no picker and no prompt path here, and the store behind it
// imports neither `openSession` nor `sendPrompt`: a child cannot be spawned or
// messaged from this view.
import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import { ArrowLeftIcon } from "@lucide/vue";
import OlderTurnsControl from "@/components/chat/OlderTurnsControl.vue";
import TurnGroup from "@/components/chat/TurnGroup.vue";
import { createTranscriptFollow } from "@/components/chat/transcript-follow";
import { deriveTurnGroups } from "@/components/chat/turn-groups";
import { useTurnWindow } from "@/components/chat/turn-window";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";
import { CONVERSATION_TRANSCRIPT_MAX_WIDTH } from "@/lib/layout";
import { showReasoning } from "@/lib/preferences";
import {
  openSubagent,
  openSubagentId,
  openSubagentStatus,
  subagentStatusDot,
  subagentStatusLabel,
  subagentTranscript,
  subagentTranscriptError,
  subagentTranscriptLoading,
} from "@/lib/subagent-store";

const emit = defineEmits<{ (e: "back"): void }>();

/** The child on screen; the panel only mounts this component when one is open. */
const child = computed(() => openSubagent.value?.child ?? null);

/** What the view calls it: the agent definition, or the run name when it has none. */
const agentName = computed(() => {
  const open = child.value;
  if (open === null) return "";
  return open.agent.trim() !== "" ? open.agent : open.name;
});

/** The run name, shown only when it says something the agent name does not. */
const runName = computed(() => {
  const open = child.value;
  if (open === null) return "";
  return open.name.trim() !== "" && open.name !== agentName.value ? open.name : "";
});

const statusLabel = computed(() => subagentStatusLabel(openSubagentStatus.value));
const dotClass = computed(() => subagentStatusDot(openSubagentStatus.value));

/**
 * The child's transcript, grouped exactly as a conversation is (S3). No chips and
 * no live turn: this view reads a file that pi's child writes, so there is nothing
 * streaming into it and no tool event ever reaches it.
 */
const groups = computed(() => deriveTurnGroups(subagentTranscript.value, [], null));

/**
 * How much of the child's history is mounted (S3): the newest page of turns plus
 * whatever the human revealed (`turn-window.ts`). A child that has run for a
 * while reads as a document rather than as a page that mounts all of it, and
 * every turn here is mounted open, so the bound is what keeps opening the view
 * cheap. Another child starts from its own newest page.
 */
const {
  groups: visibleGroups,
  hiddenCount: hiddenTurnCount,
  hasOlder: hasOlderTurns,
  showOlder: revealOlderTurns,
} = useTurnWindow(groups, () => openSubagentId.value);

/** `HH:MM` of a turn's prompt; pi's stamps, formatted for display only. */
function timeLabel(at: string | null | undefined): string | undefined {
  if (at === null || at === undefined || at === "") return undefined;
  const then = new Date(at);
  if (Number.isNaN(then.getTime())) return undefined;
  const hours = String(then.getHours()).padStart(2, "0");
  const minutes = String(then.getMinutes()).padStart(2, "0");
  return `${hours}:${minutes}`;
}

/**
 * The scrolling element, the follow controller (`transcript-follow.ts`) and the
 * transient anchor of a reveal. A view the human left at the bottom follows the
 * newest line; one they scrolled up to read is never pulled back. The scroll
 * request is coalesced to one per frame, which is what keeps a child's frequent
 * re-read from scrolling on every poll.
 */
const follow = createTranscriptFollow();
const scroller = ref<HTMLElement | null>(null);

watch(
  scroller,
  () => follow.attach(scroller.value),
  { flush: "post" },
);

/**
 * The child's file was read again and the flow changed: follow it while glued.
 * The store replaces the array only when the transcript really differs (a
 * running child keeps appending to its last message), so watching the reference
 * is what catches both a new turn and a longer one.
 */
watch(
  subagentTranscript,
  () => follow.request(),
  { flush: "post" },
);

/** Another child opens at its own newest line instead of the previous offset. */
watch(openSubagentId, () => {
  follow.jump();
  void nextTick(() => follow.syncPin());
});

/**
 * Reveals one page of older turns and keeps the reader on the line they were
 * reading: the new turns land above them, so the offset absorbs that growth.
 */
function handleShowOlderTurns(): void {
  const anchor = follow.snapshot();
  revealOlderTurns();
  void nextTick(() => follow.restore(anchor));
}

// Nothing of the view outlives it: a pending frame would write into a gone element.
onUnmounted(() => follow.detach());

/** The transcript of the child on screen; only a first read can be "loading". */
const showsLoading = computed(
  () => subagentTranscriptLoading.value && subagentTranscript.value.length === 0,
);

const isUntouched = computed(
  () =>
    !subagentTranscriptLoading.value &&
    subagentTranscript.value.length === 0 &&
    subagentTranscriptError.value === null,
);
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <header class="flex items-center gap-2 border-b border-border px-4 py-2">
      <!-- Back to the chat the human came from. It never re-selects anything: the
           parent was never left, so nothing is spawned and no process is touched. -->
      <Button
        variant="ghost"
        size="icon-sm"
        :aria-label="t('chat.subagent.back')"
        :title="t('chat.subagent.back')"
        @click="emit('back')"
      >
        <ArrowLeftIcon />
      </Button>

      <div class="flex min-w-0 flex-1 items-center gap-2">
        <h1 class="truncate text-sm font-semibold">{{ agentName }}</h1>
        <span
          class="flex shrink-0 items-center gap-1 text-[10px] text-muted-foreground"
          :title="statusLabel"
        >
          <span
            class="size-1.5 rounded-full"
            :class="dotClass"
            aria-hidden="true"
          />
          {{ statusLabel }}
        </span>
        <span
          v-if="runName !== ''"
          class="truncate text-[10px] text-muted-foreground"
        >
          · {{ runName }}
        </span>
      </div>

      <!-- The one thing this view must never be mistaken for: a chat. -->
      <span
        class="shrink-0 rounded-full border border-border px-2 py-0.5 text-[10px] text-muted-foreground"
      >
        {{ t("chat.subagent.readOnly") }}
      </span>
    </header>

    <div
      ref="scroller"
      class="min-h-0 flex-1 overflow-y-auto"
      @scroll="follow.handleScroll()"
    >
      <div
        class="mx-auto flex w-full flex-col gap-4 p-4"
        :style="{ maxWidth: `${CONVERSATION_TRANSCRIPT_MAX_WIDTH}px` }"
      >
        <Alert v-if="subagentTranscriptError" variant="destructive">
          <AlertDescription>{{ subagentTranscriptError }}</AlertDescription>
        </Alert>

        <p v-if="showsLoading" class="text-sm text-muted-foreground">
          {{ t("chat.subagent.loading") }}
        </p>
        <p v-else-if="isUntouched" class="text-sm text-muted-foreground">
          {{ t("chat.subagent.empty") }}
        </p>

        <!-- The same turn groups the conversation renders, opened instead of
             folded: nothing here is a live tail, so the work should be readable
             without a click per turn. Only the newest page is mounted; the
             control above reveals the rest. -->
        <OlderTurnsControl
          v-if="hasOlderTurns"
          :count="hiddenTurnCount"
          @reveal="handleShowOlderTurns"
        />

        <TurnGroup
          v-for="group in visibleGroups"
          :key="group.id"
          :turn-id="group.id"
          :prompt="group.prompt"
          :at="timeLabel(group.prompt?.at)"
          :question="group.question"
          :steps="group.steps"
          :tools="group.tools"
          :answer="group.answer"
          :live="false"
          :interruption="group.interruption"
          :show-reasoning="showReasoning"
          :initial-expanded="true"
        />
      </div>
    </div>

    <!-- The composer's slot, and the quiet note that explains why it is empty. -->
    <div class="px-4 py-3">
      <p class="mx-auto text-center text-xs text-muted-foreground">
        {{ t("chat.subagent.note") }}
      </p>
    </div>
  </div>
</template>
