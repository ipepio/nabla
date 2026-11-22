<script setup lang="ts">
// A question the agent asked, in the transcript (S3 of
// `odd/tasks/agent-question-bridge.md`).
//
// It has two lives and one look. While the agent is blocked on the answer the
// card is *pending*: read-only, no control, drawn where the conversation ends —
// the turn cannot continue until the human answers, so a question that only
// appeared in the composer slot left the conversation looking finished while pi
// was in fact waiting. Once the answer lands, the same card becomes *settled*
// (the ✓ on the chosen option, the typed answer, or the `Cancelled` badge), and
// it lands in exactly the slot the pending one occupied: the transcript entry
// that carries the `SettledQuestion` is the last message, which is where this
// card was.
//
// Read-only by construction in both states: it has no emit, draws no answer
// control and cannot be answered. The composer slot (`QuestionPrompt.vue`) is
// the only interactive half of the exchange.
//
// The strings are literal English rather than `t(...)` keys: the catalogue
// (`src/locales/`) is out of this feature's edit surface, and inventing keys the
// catalogue does not carry would break `vue-tsc` (a missing key is a build error).
import { computed } from "vue";
import { CheckIcon, MessageCircleQuestionIcon } from "@lucide/vue";
import type { UiAnswer } from "@/contracts/types";
import type { PendingQuestion, SettledQuestion } from "@/lib/session-store";

const props = defineProps<{
  /**
   * The question as pi sent it, plus what the human did with it once it is
   * history. The live shape (`PendingQuestion`) has no `answer` field at all,
   * which is the one difference this card reads.
   */
  question: PendingQuestion | SettledQuestion;
}>();

/** The agent is still blocked on this one: there is no answer to show yet. */
const pending = computed(() => !("answer" in props.question));

/** The question itself, falling back to what it was asking about. */
const headline = computed(() => {
  if (props.question.title !== "") return props.question.title;
  if (props.question.message !== null && props.question.message !== "") {
    return props.question.message;
  }
  return pending.value
    ? "The agent has a question."
    : "The agent asked a question.";
});

/** What the human answered; `null` while pending and for a cancellation. */
const answer = computed<UiAnswer | null>(() =>
  "answer" in props.question ? props.question.answer : null,
);

/**
 * The answer in one line: the chosen option, `Yes`/`No`, or the typed text.
 * `""` when the question was cancelled — nothing was answered then — and while
 * it is still pending.
 */
const answerText = computed(() => {
  const given = answer.value;
  if (given === null || "cancelled" in given) return "";
  if ("confirmed" in given) return given.confirmed ? "Yes" : "No";
  return given.value;
});

/** The question was closed without an answer. */
const cancelled = computed(() => {
  const given = answer.value;
  return given === null || "cancelled" in given;
});

/**
 * The options the question offered. A `confirm` has no option list on the wire —
 * its two answers are fixed, so they are minted here — and anything else (a
 * typed answer, or a method with no control here) offers none, so the answer is
 * printed as a line instead.
 *
 * While the question is pending only the real options are listed: nothing is
 * choosable on this card, and minting a `Yes`/`No` pair here would read as two
 * controls the human cannot press (the composer slot already asks them).
 */
const choices = computed<string[]>(() => {
  if (props.question.options.length > 0) return props.question.options;
  if (pending.value) return [];
  return props.question.method === "confirm" ? ["Yes", "No"] : [];
});
</script>

<template>
  <div
    class="nabla-enter rounded-xl border bg-card p-3.5"
    :class="pending ? 'border-dashed border-primary/40' : 'border-border'"
  >
    <div class="flex items-start gap-2.5">
      <MessageCircleQuestionIcon
        class="mt-0.5 size-4 shrink-0 text-tertiary"
        aria-hidden="true"
      />
      <div class="min-w-0 flex-1">
        <p class="text-[13px] font-semibold break-words whitespace-pre-wrap text-foreground">
          {{ headline }}
        </p>
        <!-- `confirm` carries what exactly was confirmed; `input` carries the
             hint its empty field showed. -->
        <p
          v-if="question.message"
          class="mt-1 text-xs break-words whitespace-pre-wrap text-muted-foreground"
        >
          {{ question.message }}
        </p>
        <p
          v-else-if="question.placeholder"
          class="mt-1 text-xs break-words whitespace-pre-wrap text-muted-foreground"
        >
          {{ question.placeholder }}
        </p>

        <!-- The options, with the one the human picked marked. A pending
             question marks none — nothing has been chosen — and a cancelled one
             marks none either: nothing will be. -->
        <ul v-if="choices.length > 0" class="mt-2 flex flex-col gap-1">
          <li
            v-for="choice in choices"
            :key="choice"
            class="flex items-center gap-1.5 text-xs"
            :class="
              choice === answerText ? 'font-medium text-primary' : 'text-muted-foreground'
            "
          >
            <CheckIcon
              v-if="!pending && choice === answerText"
              class="size-3.5 shrink-0 text-primary"
              aria-hidden="true"
            />
            <span
              v-else-if="!pending"
              class="size-3.5 shrink-0"
              aria-hidden="true"
            />
            <span class="min-w-0 break-words">{{ choice }}</span>
          </li>
        </ul>
        <!-- A typed answer has no option to mark, so it is the line itself. -->
        <p
          v-else-if="answerText !== ''"
          class="mt-2 text-xs font-medium break-words whitespace-pre-wrap text-primary"
        >
          {{ answerText }}
        </p>

        <!-- The pending card cannot be answered from here; saying where the
             answer goes is the whole job of this line. -->
        <p
          v-if="pending"
          class="mt-2 font-mono text-[10px] font-medium tracking-wider text-muted-foreground uppercase"
        >
          Waiting for your answer
        </p>
      </div>

      <!-- How the exchange ended, at a glance. A pending question has no ending
           yet, so it carries no badge — the hint above already says it waits. -->
      <span
        v-if="!pending"
        class="shrink-0 rounded border px-1.5 py-0.5 font-mono text-[10px] font-semibold tracking-wide uppercase"
        :class="
          cancelled
            ? 'border-border bg-secondary text-muted-foreground'
            : 'border-primary/25 bg-primary/10 text-primary'
        "
      >
        {{ cancelled ? "Cancelled" : "Answered" }}
      </span>
    </div>
  </div>
</template>
