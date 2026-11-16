<script setup lang="ts">
// The reasoning pi streamed for one turn, collapsed by default — pi's own
// setting keeps reasoning hidden, so the transcript follows it — and open only
// when a human opened it.
//
// The open/closed state is not component state: it lives in the session store,
// keyed by message id, so it survives re-renders and switching sessions and is
// dropped with the session. A streaming turn shares the id its message flushes
// into, so a block opened mid-stream stays open once the turn completes.
import { computed } from "vue";
import { ChevronRightIcon } from "@lucide/vue";
import { t } from "@/lib/i18n";
import { isReasoningExpanded, toggleReasoning } from "@/lib/session-store";

const props = defineProps<{
  /** The message the block belongs to; the key of its open/closed state. */
  messageId: string;
  /** Reasoning text, plain: no code rendering, whitespace preserved. */
  text: string;
  /** The turn is still streaming reasoning into this block. */
  live?: boolean;
}>();

/** DOM id of the collapsible region, derived from a store id (`12:3`), which is
 * not a safe id as-is. */
const contentId = computed(
  () => `reasoning-${props.messageId.replace(/[^a-zA-Z0-9_-]/g, "-")}`,
);

/** Collapsed unless the store remembers this message being opened. */
const expanded = computed(() => isReasoningExpanded(props.messageId));

function toggle() {
  toggleReasoning(props.messageId);
}
</script>

<template>
  <div class="rounded-md border border-border/70 bg-muted/20">
    <button
      type="button"
      class="flex w-full items-center gap-1.5 px-2.5 py-1.5 text-left text-xs text-muted-foreground transition-colors hover:text-foreground"
      :aria-expanded="expanded"
      :aria-controls="contentId"
      @click="toggle"
    >
      <ChevronRightIcon
        class="size-3 shrink-0 transition-transform"
        :class="{ 'rotate-90': expanded }"
        aria-hidden="true"
      />
      <span class="font-medium">{{ t("chat.reasoning.label") }}</span>
      <span
        v-if="live"
        class="size-1.5 shrink-0 animate-pulse rounded-full bg-muted-foreground/60"
        aria-hidden="true"
      />
    </button>
    <!-- Bounded height with its own scroll: a long chain of thought must not
         push the answer off screen, and scrolling inside it must not scroll the
         transcript (overscroll-contain). -->
    <div
      v-show="expanded"
      :id="contentId"
      class="mr-2.5 mb-2 ml-2.5 max-h-56 overflow-y-auto overscroll-contain border-l-2 border-border py-0.5 pr-1 pl-2.5 text-xs leading-relaxed whitespace-pre-wrap text-muted-foreground"
    >
      {{ text }}
    </div>
  </div>
</template>
