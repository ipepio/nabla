<script setup lang="ts">
// One transcript entry (spec S2): user turns are right-aligned bubbles with a
// timestamp and a copy control; assistant turns are full-width prose, so
// streamed deltas read like a document rather than a chat log.
//
// Assistant turns render their reasoning above the answer — collapsed until the
// human opens it — and their text through `AssistantText`, which turns fenced
// code into copyable blocks without a markdown dependency.
import { ref } from "vue";
import { CheckIcon, CopyIcon } from "@lucide/vue";
import AssistantText from "@/components/chat/AssistantText.vue";
import ReasoningBlock from "@/components/chat/ReasoningBlock.vue";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";

const props = defineProps<{
  role: "user" | "assistant";
  text: string;
  /** `HH:MM`, user turns only. */
  at?: string;
  /** The turn's reasoning, when pi streamed any. */
  reasoning?: string;
  /** Stable message id: keys the reasoning block's open/closed state. */
  messageId?: string;
  /** The turn is still streaming; the reasoning block says so. */
  reasoningLive?: boolean;
}>();

const copied = ref(false);
let resetHandle: number | null = null;

async function copyText() {
  try {
    await navigator.clipboard.writeText(props.text);
    copied.value = true;
    if (resetHandle !== null) window.clearTimeout(resetHandle);
    resetHandle = window.setTimeout(() => {
      copied.value = false;
      resetHandle = null;
    }, 1500);
  } catch {
    // A denied clipboard write leaves the control in its idle state; there is
    // no toast layer yet to report it.
    copied.value = false;
  }
}
</script>

<template>
  <div v-if="role === 'user'" class="flex flex-col items-end gap-1">
    <div
      class="max-w-[75%] rounded-2xl bg-secondary px-3.5 py-2 text-sm whitespace-pre-wrap text-secondary-foreground"
    >
      {{ text }}
    </div>
    <div class="flex items-center gap-1 text-xs text-muted-foreground">
      <span v-if="at">{{ at }}</span>
      <Button
        variant="ghost"
        size="icon-xs"
        :aria-label="t('message.copy')"
        @click="copyText"
      >
        <CheckIcon v-if="copied" />
        <CopyIcon v-else />
      </Button>
    </div>
  </div>

  <div v-else class="flex flex-col gap-2 text-sm text-foreground">
    <!-- Above the answer, per spec: reasoning is context, not part of the reply. -->
    <ReasoningBlock
      v-if="reasoning !== undefined && reasoning !== '' && messageId !== undefined"
      :message-id="messageId"
      :text="reasoning"
      :live="reasoningLive"
    />
    <!-- Immediately under the reasoning block, the turn's other piece of folded
         context: its tool activity, one collapsed row (see `ToolSummary`). The
         slot is empty for a turn that called no tool and for a live turn, whose
         chips render expanded below the transcript instead. -->
    <slot name="tools" />
    <AssistantText v-if="text !== ''" :text="text" />
  </div>
</template>
