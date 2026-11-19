<script setup lang="ts">
// One pending question the session's agent is blocked on (S3 of
// `odd/tasks/agent-question-bridge.md`).
//
// Presentation only: it renders one `extension_ui_request` and emits the answer
// the human gave. The command that sends it and the store that removes the card
// live in `ChatPanel.vue` and `src/lib/session-store.ts`, so this component never
// touches the contracts seam.
//
// Every method this slice draws has its own control. A method it does not draw
// (`editor`, or whatever a newer pi adds) still offers Cancel: a card nobody can
// answer would block the agent's turn forever (S4).
//
// The strings are literal English rather than `t(...)` keys: the catalogue
// (`src/locales/`) is out of this feature's edit surface, and inventing keys the
// catalogue does not carry would break `vue-tsc` (a missing key is a build error).
import { computed, ref } from "vue";
import { MessageCircleQuestionIcon, SendIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { UiAnswer } from "@/contracts/types";
import type { PendingQuestion } from "@/lib/session-store";

const props = defineProps<{
  /** The request, normalized by the store (no optional fields to guard). */
  question: PendingQuestion;
  /** An answer of this session is already on its way: every card is inert. */
  busy: boolean;
}>();

const emit = defineEmits<{ (e: "answer", answer: UiAnswer): void }>();

/** The `input` method's text. Per card, so two pending inputs never share it. */
const text = ref("");

/** Methods this card can draw; anything else can only be cancelled. */
const supported = computed(
  () =>
    props.question.method === "select" ||
    props.question.method === "confirm" ||
    props.question.method === "input",
);

/** Sends the typed text, never an empty one: an empty value is not an answer. */
function answerInput(): void {
  const value = text.value.trim();
  if (value === "" || props.busy) return;
  emit("answer", { value });
}
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
          {{ question.title || "The agent has a question." }}
        </p>
        <!-- `confirm` carries what exactly is being confirmed. -->
        <p
          v-if="question.message"
          class="mt-1 text-xs break-words whitespace-pre-wrap text-muted-foreground"
        >
          {{ question.message }}
        </p>
      </div>
    </div>

    <!-- The method's own control. -->
    <div class="mt-2 flex flex-col gap-1.5">
      <div v-if="question.method === 'select'" class="flex flex-wrap gap-1.5">
        <Button
          v-for="option in question.options"
          :key="option"
          variant="outline"
          size="sm"
          :disabled="busy"
          @click="emit('answer', { value: option })"
        >
          {{ option }}
        </Button>
      </div>
      <div v-else-if="question.method === 'confirm'" class="flex flex-wrap gap-1.5">
        <Button size="sm" :disabled="busy" @click="emit('answer', { confirmed: true })">
          Yes
        </Button>
        <Button
          variant="outline"
          size="sm"
          :disabled="busy"
          @click="emit('answer', { confirmed: false })"
        >
          No
        </Button>
      </div>
      <div v-else-if="question.method === 'input'" class="flex items-center gap-1.5">
        <Input
          v-model="text"
          class="h-8 text-sm"
          :placeholder="question.placeholder ?? 'Type your answer'"
          :disabled="busy"
          @keydown.enter.prevent="answerInput"
        />
        <Button
          size="icon-sm"
          aria-label="Send answer"
          :disabled="busy || text.trim() === ''"
          @click="answerInput"
        >
          <SendIcon />
        </Button>
      </div>
      <p v-else-if="!supported" class="text-xs text-muted-foreground">
        This question has no control in the chat. Cancel it so the agent can carry
        on.
      </p>
    </div>

    <!-- Cancel is not optional: without it a pending question can only be
         closed by stopping the whole session (S4). -->
    <div class="mt-1.5 flex items-center justify-end">
      <Button
        variant="ghost"
        size="xs"
        :disabled="busy"
        @click="emit('answer', { cancelled: true })"
      >
        Cancel
      </Button>
    </div>
  </div>
</template>