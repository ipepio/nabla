<script setup lang="ts">
// One pending question, rendered in the composer slot instead of the transcript
// (S3 of `odd/tasks/agent-question-bridge.md`).
//
// The design decision this file implements: a question takes the place the
// human's hands are already in. `select` and `confirm` replace the input row with
// a list the arrow keys walk and Enter confirms from; `input` turns that row into
// the answer field, with the question's own placeholder. Either way Escape
// cancels and a visible Cancel keeps that discoverable.
//
// Presentation only: it renders one `PendingQuestion`, holds the highlight and
// the focus, and emits what the human decided. The command that answers pi lives
// in `ChatPanel.vue`, the pending stack in `src/lib/session-store.ts`.
//
// Every key is handled at this container — never on `window` — so a session that
// is off screen can never answer a keystroke meant for the one on screen. The
// `input` field's own Enter bubbles up here, which is why one handler covers the
// picker and the field alike.
//
// The strings are literal English rather than `t(...)` keys: the catalogue
// (`src/locales/`) is out of this feature's edit surface, and inventing keys it
// does not carry would break `vue-tsc` (a missing key is a build error).
import { computed, nextTick, onMounted, ref } from "vue";
import { ArrowUpIcon, MessageCircleQuestionIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { UiAnswer } from "@/contracts/types";
import { t } from "@/lib/i18n";
import { COMPOSER_MIN_WIDTH } from "@/lib/layout";
import type { PendingQuestion } from "@/lib/session-store";

const props = defineProps<{
  /** The question on screen: the oldest pending one, or the one Tab walked to. */
  question: PendingQuestion;
  /** The answer being typed for an `input` question (`v-model`). */
  modelValue: string;
  /** Position of this question among the pending ones, 1-based. */
  position: number;
  /** How many questions are pending for this session. */
  total: number;
  /** An answer is already on its way: every control here is inert. */
  busy: boolean;
  /**
   * Ceiling for the card, chosen by the caller from `src/lib/layout.ts`, so the
   * question lands exactly where the composer it replaces would have been.
   */
  maxWidth: number;
}>();

const emit = defineEmits<{
  (e: "update:modelValue", value: string): void;
  (e: "answer", answer: UiAnswer): void;
  (e: "cancel"): void;
  /** Tab / Shift+Tab: the next or previous pending question of this session. */
  (e: "next"): void;
  (e: "previous"): void;
}>();

/** The question itself, falling back to what it was asking about. */
const headline = computed(() => {
  if (props.question.title !== "") return props.question.title;
  if (props.question.message !== null && props.question.message !== "") {
    return props.question.message;
  }
  return "The agent has a question.";
});

/**
 * A question answered by choosing rather than by typing: `select` offers its
 * options, `confirm` offers the two fixed answers. An unknown method falls on the
 * other side, so a request this panel cannot draw still gets a field and a way to
 * cancel instead of a dead end.
 */
const offersChoices = computed(
  () => props.question.options.length > 0 || props.question.method === "confirm",
);

/**
 * The rows the picker draws, in pi's order. `confirm` has no option list on the
 * wire — its answers are the protocol's own `Yes`/`No` — so they are minted here,
 * and only here: `toAnswer` is what turns a row back into the wire shape.
 */
const choices = computed<string[]>(() => {
  if (props.question.options.length > 0) return props.question.options;
  if (props.question.method === "confirm") return ["Yes", "No"];
  return [];
});

/**
 * The highlighted row. The first one when the question appears, which is the
 * documented default (`↑`/`↓` start from the top) and what makes "open it and
 * press Enter" answer the first option.
 *
 * It is reset by the caller remounting this component for another question
 * (`:key` in ChatPanel), so the highlight can never arrive already walked.
 */
const highlight = ref(0);

/** The submit control is live only when the click has somewhere to go. */
const canSubmit = computed(() => {
  if (props.busy) return false;
  if (offersChoices.value) return choices.value.length > 0;
  return props.modelValue.trim() !== "";
});

/** The keys that work right now, spelled out: the picker must not be a secret. */
const hint = computed(() => {
  const walk = props.total > 1 ? "Tab next question · " : "";
  return offersChoices.value
    ? `↑↓ move · Enter choose · ${walk}Esc cancel`
    : `Enter answer · ${walk}Esc cancel`;
});

/**
 * A choice row becomes the wire answer the method defines: `confirm` answers with
 * a boolean, every other choice with the option's own text.
 */
function toAnswer(choice: string): UiAnswer {
  return props.question.method === "confirm"
    ? { confirmed: choice === "Yes" }
    : { value: choice };
}

/** Answers with one row of the picker. */
function choose(index: number): void {
  if (props.busy) return;
  const choice = choices.value[index];
  if (choice === undefined) return;
  emit("answer", toAnswer(choice));
}

/** Moves the highlight, wrapping at both ends like a terminal menu. */
function move(delta: number): void {
  const count = choices.value.length;
  if (count === 0) return;
  highlight.value = (highlight.value + delta + count) % count;
}

/**
 * Enter and the submit control share this, so they can never disagree: the
 * picker answers the highlighted row, the field answers what it holds. An empty
 * field is not an answer and sends nothing.
 */
function submit(): void {
  if (props.busy) return;
  if (offersChoices.value) {
    choose(highlight.value);
    return;
  }
  const value = props.modelValue.trim();
  if (value === "") return;
  emit("answer", { value });
}

function handleKeydown(event: KeyboardEvent): void {
  if (props.busy) return;
  switch (event.key) {
    case "Escape":
      event.preventDefault();
      emit("cancel");
      return;
    case "Enter":
      // Shift+Enter is a line break the human typed, not a submit.
      if (event.shiftKey) return;
      event.preventDefault();
      submit();
      return;
    case "Tab":
      // More than one pending question: Tab walks the stack instead of leaving
      // the panel. A single question has nowhere to walk to, so focus moves on
      // exactly as it did before.
      if (props.total < 2) return;
      event.preventDefault();
      if (event.shiftKey) {
        emit("previous");
        return;
      }
      emit("next");
      return;
    case "ArrowUp":
    case "ArrowDown":
      // Only the picker owns the arrows; the field keeps its own cursor keys.
      if (!offersChoices.value) return;
      event.preventDefault();
      move(event.key === "ArrowDown" ? 1 : -1);
      return;
    default:
      return;
  }
}

const root = ref<HTMLElement | null>(null);
/** The wrapper of the answer field, so the focus lands on the input itself. */
const field = ref<HTMLElement | null>(null);

/**
 * Keeps the keyboard on the card after a click on something that cannot hold the
 * focus (the title, the hint line). A control inside keeps its own focus; a click
 * on the surrounding card would otherwise blur to the body and silently disable
 * the arrows, Escape and Enter the picker is built around.
 */
function keepFocus(event: MouseEvent): void {
  const target = event.target;
  if (
    target instanceof HTMLElement &&
    target.closest("button, input, textarea") !== null
  ) {
    return;
  }
  root.value?.focus();
}

/**
 * The control that owns the answer takes the focus the moment it appears: the
 * list for a picker, the text field for an `input` question. This runs on mount
 * only — the caller remounts this component when the focused question changes —
 * so it never steals focus from a field the human is already typing in.
 */
function focusOwnControl(): void {
  void nextTick(() => {
    if (offersChoices.value) {
      root.value?.focus();
      return;
    }
    field.value?.querySelector("input")?.focus();
  });
}

onMounted(focusOwnControl);
</script>

<template>
  <!-- Same card the composer draws, in the same place: the question is not a new
       surface, it is what the input row becomes while the agent waits. -->
  <div
    ref="root"
    class="mx-auto w-full min-w-0 rounded-2xl border border-border bg-card/60 shadow-sm outline-none focus-visible:border-ring"
    :style="{ maxWidth: `${maxWidth}px`, minWidth: `${COMPOSER_MIN_WIDTH}px` }"
    role="group"
    :aria-label="headline"
    tabindex="-1"
    @keydown="handleKeydown"
    @click="keepFocus"
  >
    <div class="flex items-start gap-2 px-3.5 pt-2.5">
      <MessageCircleQuestionIcon
        class="mt-0.5 size-3.5 shrink-0 text-muted-foreground"
        aria-hidden="true"
      />
      <div class="min-w-0 flex-1">
        <p class="text-sm font-medium break-words whitespace-pre-wrap">
          {{ headline }}
        </p>
        <!-- `confirm` carries what exactly is being confirmed. -->
        <p
          v-if="question.message"
          class="mt-1 text-xs break-words whitespace-pre-wrap text-muted-foreground"
        >
          {{ question.message }}
        </p>
      </div>
      <!-- Several questions pending: where this one sits in the stack. -->
      <span
        v-if="total > 1"
        class="shrink-0 text-[10px] tabular-nums text-muted-foreground"
        :aria-label="`Question ${position} of ${total}`"
      >
        {{ position }} / {{ total }}
      </span>
    </div>

    <!-- The picker: the agent's options as rows, the highlighted one ready to be
         confirmed with Enter. -->
    <ul
      v-if="choices.length > 0"
      role="listbox"
      class="mt-2 flex max-h-48 cursor-pointer flex-col gap-0.5 overflow-y-auto px-2"
      :aria-label="headline"
    >
      <!-- Each row is the option itself (`tabindex="-1"`): the arrows walk the
           list, so Tab stays free for the pending-question stack and never lands
           on a row. -->
      <li
        v-for="(choice, index) in choices"
        :key="choice"
        role="option"
        tabindex="-1"
        :aria-selected="index === highlight"
        :aria-disabled="busy"
        class="flex w-full items-center gap-2 rounded-md px-2 py-1 text-sm"
        :class="
          index === highlight
            ? 'bg-muted text-foreground'
            : 'text-muted-foreground hover:bg-muted/60'
        "
        @click="choose(index)"
        @mouseenter="highlight = index"
      >
        <span class="w-3 shrink-0 text-[10px] tabular-nums" aria-hidden="true">
          {{ index === highlight ? "›" : "" }}
        </span>
        <span class="min-w-0 flex-1 break-words">{{ choice }}</span>
      </li>
    </ul>

    <!-- A `select` that carried no options has nothing to pick from; cancelling
         is the only way forward, and saying so is better than an empty box. -->
    <p
      v-else-if="offersChoices"
      class="mt-2 px-3.5 text-xs text-muted-foreground"
    >
      This question offers no options. Cancel it so the agent can carry on.
    </p>

    <!-- `ask_input`: the composer *is* the answer field. -->
    <div v-else ref="field" class="mt-2 px-3.5">
      <Input
        :model-value="modelValue"
        :placeholder="question.placeholder ?? 'Type your answer'"
        class="h-9 text-sm"
        :disabled="busy"
        @update:model-value="emit('update:modelValue', String($event))"
      />
    </div>

    <div class="mt-2 flex items-center gap-1 px-2 pb-2">
      <!-- The keyboard is the point of the picker, so it is written down. -->
      <p
        class="hidden min-w-0 flex-1 truncate px-1.5 text-[10px] text-muted-foreground sm:block"
      >
        {{ hint }}
      </p>
      <div class="ms-auto flex shrink-0 items-center gap-1.5">
        <!-- The visible half of Escape: a question nobody can close would block
             the agent's turn forever. -->
        <Button variant="ghost" size="sm" :disabled="busy" @click="emit('cancel')">
          {{ t("common.cancel") }}
        </Button>
        <Button
          size="icon-sm"
          class="rounded-full"
          :aria-label="offersChoices ? 'Choose option' : 'Answer question'"
          :disabled="!canSubmit"
          @click="submit"
        >
          <ArrowUpIcon />
        </Button>
      </div>
    </div>
  </div>
</template>
