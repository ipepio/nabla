<script setup lang="ts">
// A question the agent asked, once it is history (S3 of
// `odd/tasks/agent-question-bridge.md`).
//
// Pending questions do not render here any more: they take over the composer
// slot (`QuestionPrompt.vue`), which is where the human's hands already are.
// This card is what a question becomes when it is answered or cancelled, so the
// transcript keeps both halves of the exchange after the request is gone — the
// entry is a normal `ChatMessage` carrying a `SettledQuestion`, never a second
// message kind.
//
// Read-only by construction: it has no emit, draws no control and cannot be
// answered, because the protocol exchange it belongs to is already closed.
//
// The strings are literal English rather than `t(...)` keys: the catalogue
// (`src/locales/`) is out of this feature's edit surface, and inventing keys the
// catalogue does not carry would break `vue-tsc` (a missing key is a build error).
import { computed } from "vue";
import { CheckIcon, MessageCircleQuestionIcon } from "@lucide/vue";
import type { SettledQuestion } from "@/lib/session-store";

const props = defineProps<{
  /** The request as pi sent it, plus what the human did with it. */
  question: SettledQuestion;
}>();

/** The question itself, falling back to what it was asking about. */
const headline = computed(() => {
  if (props.question.title !== "") return props.question.title;
  if (props.question.message !== null && props.question.message !== "") {
    return props.question.message;
  }
  return "The agent asked a question.";
});

/**
 * The answer in one line: the chosen option, `Yes`/`No`, or the typed text.
 * `""` when the question was cancelled — nothing was answered then.
 */
const answerText = computed(() => {
  const answer = props.question.answer;
  if (answer === null || "cancelled" in answer) return "";
  if ("confirmed" in answer) return answer.confirmed ? "Yes" : "No";
  return answer.value;
});

/** The question was closed without an answer. */
const cancelled = computed(
  () => props.question.answer === null || "cancelled" in props.question.answer,
);

/**
 * The options the question offered, with the chosen one marked. A `confirm` has
 * no option list on the wire — its two answers are fixed — and anything else
 * (a typed answer, or a method with no control here) offers none, so the answer
 * is printed as a line instead.
 */
const choices = computed<string[]>(() => {
  if (props.question.options.length > 0) return props.question.options;
  if (props.question.method === "confirm") return ["Yes", "No"];
  return [];
});
</script>

<template>
  <div class="rounded-lg border border-border bg-muted/30 p-2.5">
    <div class="flex items-start gap-2">
      <MessageCircleQuestionIcon
        class="mt-0.5 size-3.5 shrink-0 text-muted-foreground"
        aria-hidden="true"
      />
      <div class="min-w-0 flex-1">
        <p class="text-sm font-medium break-words whitespace-pre-wrap">
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

        <!-- The options, with the one the human picked marked. A cancelled
             question marks none of them: nothing was chosen. -->
        <ul v-if="choices.length > 0" class="mt-1.5 flex flex-col gap-0.5">
          <li
            v-for="choice in choices"
            :key="choice"
            class="flex items-center gap-1.5 text-xs"
            :class="
              choice === answerText ? 'text-foreground' : 'text-muted-foreground'
            "
          >
            <CheckIcon
              v-if="choice === answerText"
              class="size-3 shrink-0"
              aria-hidden="true"
            />
            <span v-else class="size-3 shrink-0" aria-hidden="true" />
            <span class="min-w-0 break-words">{{ choice }}</span>
          </li>
        </ul>
        <!-- A typed answer has no option to mark, so it is the line itself. -->
        <p
          v-else-if="answerText !== ''"
          class="mt-1.5 text-xs break-words whitespace-pre-wrap text-foreground"
        >
          {{ answerText }}
        </p>
      </div>

      <!-- How the exchange ended, at a glance. -->
      <span
        class="shrink-0 text-[10px] font-medium tracking-wider uppercase"
        :class="cancelled ? 'text-muted-foreground' : 'text-success'"
      >
        {{ cancelled ? "Cancelled" : "Answered" }}
      </span>
    </div>
  </div>
</template>
