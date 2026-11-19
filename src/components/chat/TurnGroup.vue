<script setup lang="ts">
// One human turn, everything pi did for it, inside one frame (S1 of
// `chat-turn-groups.md`). Top to bottom:
//
//   1. the human's bubble (or the settled question card that opened the turn);
//   2. one disclosure holding the turn's finished steps — each step's reasoning,
//      its text and its tool chips, in the order they happened — with the step
//      count and the failure count on the collapsed line (S4);
//   3. the turn's final answer, visible without opening anything (S2).
//
// A turn still running (S3) keeps its finished steps inside that disclosure and
// renders the step in progress outside it: the reasoning streaming right now,
// the chips of that step, and the answer as it is written. When the turn ends and
// the next one starts, everything folds except the answer.
import { computed } from "vue";
import { WrenchIcon } from "@lucide/vue";
import AssistantText from "@/components/chat/AssistantText.vue";
import MessageBubble from "@/components/chat/MessageBubble.vue";
import QuestionCard from "@/components/chat/QuestionCard.vue";
import ReasoningBlock from "@/components/chat/ReasoningBlock.vue";
import ToolChips from "@/components/chat/ToolChips.vue";
import ToolSummary from "@/components/chat/ToolSummary.vue";
import type { TurnStep } from "@/components/chat/turn-groups";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";
import type {
  ChatMessage,
  SettledQuestion,
  ToolEntry,
  TurnInterruption,
} from "@/lib/session-store";

const props = defineProps<{
  /** The turn's id; keys the disclosure's DOM id. */
  turnId: string;
  /** The human message that opened the turn, when it had one. */
  prompt: ChatMessage | null;
  /** `HH:MM` of the prompt, user turns only. */
  at?: string;
  /** A settled question that opens the turn, rendered as its card instead. */
  question: SettledQuestion | null;
  /** The turn's finished steps, oldest first. */
  steps: TurnStep[];
  /** Every chip of the turn, for the collapsed line. */
  tools: ToolEntry[];
  /** The turn's final answer; absent while the turn is still running. */
  answer: ChatMessage | null;
  /** The turn is being answered right now (S3). */
  live: boolean;
  /** Text streaming into the live turn's answer. */
  liveText?: string;
  /** Reasoning streaming into the live turn's current step. */
  liveReasoning?: string;
  /** Id of the message the live step will flush into; keys its open state. */
  liveMessageId?: string | null;
  /** Chips of the step in progress, which no committed message owns yet. */
  liveTools?: ToolEntry[];
  /** The "show reasoning" preference: off means the blocks are not rendered. */
  showReasoning: boolean;
  /**
   * Why this turn did not end on its own (S3 of `turn-continuity.md`), or `null`
   * for a turn that ended normally. The store reads it from pi's own
   * `stopReason`, so nothing here is inferred.
   */
  interruption: TurnInterruption | null;
}>();

/** S5: the human decided to continue the turn that was cut short. */
const emit = defineEmits<{ (e: "continue"): void }>();

/** What the collapsed line counts: the step it opens to (S1, S4). */
const stepsLabel = computed(() =>
  t("chat.turn.steps", { count: props.steps.length }),
);

/** The disclosure exists when the turn did any work at all. */
const hasWork = computed(
  () => props.steps.length > 0 || props.tools.length > 0,
);

/** A step's reasoning, or `null` when it streamed none: also the `v-if` test. */
function reasoningOf(step: TurnStep): string | null {
  const reasoning = step.message?.reasoning;
  return reasoning === undefined || reasoning === "" ? null : reasoning;
}

/**
 * The text a step wrote, or `null` when it belongs outside the fold.
 *
 * The turn's answer is rendered below the disclosure (S2), so its own copy inside
 * would be the same paragraph twice. Every other step's words are part of the
 * "how" — they say what pi was doing between two tool calls — so they stay inside,
 * in order, above that step's chips.
 */
function intermediateTextOf(step: TurnStep): string | null {
  const message = step.message;
  if (message === null || message.text === "") return null;
  if (props.answer !== null && props.answer.id === message.id) return null;
  return message.text;
}

/** The live step's reasoning / chips, only while the turn is running. */
const liveReasoningText = computed(() => props.liveReasoning ?? "");
const liveChipList = computed(() => (props.live ? (props.liveTools ?? []) : []));
const liveAnswerText = computed(() => (props.live ? (props.liveText ?? "") : ""));

/**
 * S3/S4 of `turn-continuity.md`: the line a cut turn says about itself, or
 * `null` for a turn that ended on its own.
 *
 * Three reasons are announced, each in its own words — the human stopped it, the
 * answer ran out of room, or something failed, with pi's own explanation when it
 * sent one. `stop`, `toolUse`, `pending` and `deferred` are a normal turn and say
 * nothing, and a reason this build does not know is not guessed into a notice.
 */
const notice = computed<string | null>(() => {
  const interruption = props.interruption;
  if (interruption === null) return null;
  switch (interruption.stopReason) {
    case "aborted":
      return t("chat.turn.aborted");
    case "length":
      return t("chat.turn.length");
    case "error":
      return interruption.errorMessage === null
        ? t("chat.turn.errorUnknown")
        : t("chat.turn.error", { message: interruption.errorMessage });
    default:
      return null;
  }
});
</script>

<template>
  <div class="flex flex-col gap-3">
    <!-- S1: the turn opens with what the human said. A settled question is the
         human's side of the exchange, so it opens one too, as its card. -->
    <QuestionCard v-if="question !== null" :question="question" />
    <MessageBubble
      v-else-if="prompt !== null"
      role="user"
      :text="prompt.text"
      :at="at"
    />

    <!-- S1: one disclosure for the turn's work, collapsed by default. S4: the
         failure count stays on the collapsed line, in the destructive colour. -->
    <ToolSummary
      v-if="hasWork"
      :turn-id="turnId"
      :label="stepsLabel"
      :tools="tools"
    >
      <div class="flex flex-col gap-3">
        <div
          v-for="step in steps"
          :key="step.id"
          class="flex flex-col gap-2"
        >
          <!-- Reasoning first: it is what led to the chips under it. -->
          <ReasoningBlock
            v-if="showReasoning && reasoningOf(step) !== null"
            :message-id="step.id"
            :text="reasoningOf(step) ?? ''"
          />
          <AssistantText
            v-if="intermediateTextOf(step) !== null"
            :text="intermediateTextOf(step) ?? ''"
          />
          <ToolChips v-if="step.tools.length > 0" :tools="step.tools" />
          <!-- A `tool` line of a recovered transcript: the muted note it has
               always been, kept next to the step it belongs to. -->
          <div
            v-for="note in step.notes"
            :key="note.id"
            class="flex items-start gap-2 text-xs text-muted-foreground"
          >
            <WrenchIcon class="mt-0.5 size-3 shrink-0" aria-hidden="true" />
            <span class="min-w-0 whitespace-pre-wrap">{{ note.text }}</span>
          </div>
        </div>
      </div>
    </ToolSummary>

    <!-- S3: the step running right now stays outside the fold — its reasoning and
         its chips are the informative moment, not folded context. -->
    <template v-if="live">
      <ReasoningBlock
        v-if="showReasoning && liveReasoningText !== '' && liveMessageId"
        :message-id="liveMessageId"
        :text="liveReasoningText"
        live
      />
      <ToolChips v-if="liveChipList.length > 0" :tools="liveChipList" />
    </template>

    <!-- S2: the answer, above everything else the turn did. While the turn runs
         this is the tail being written; afterwards it is the committed message. -->
    <MessageBubble
      v-if="live && liveAnswerText !== ''"
      role="assistant"
      :text="liveAnswerText"
    />
    <MessageBubble
      v-else-if="answer !== null"
      role="assistant"
      :text="answer.text"
    />

    <!-- S3/S4 of `turn-continuity.md`: the turn did not end on its own, so it
         says why, inside its own group and without alarmism. S5: the one action
         that makes sense is to continue where it stopped, which is a new turn and
         not a re-run of the prompt above. -->
    <div
      v-if="notice !== null"
      class="flex items-center justify-between gap-3 rounded-md border border-border px-3 py-2"
    >
      <p class="min-w-0 text-xs text-muted-foreground">{{ notice }}</p>
      <Button
        variant="outline"
        size="xs"
        class="shrink-0"
        @click="emit('continue')"
      >
        {{ t("chat.turn.continue") }}
      </Button>
    </div>
  </div>
</template>
