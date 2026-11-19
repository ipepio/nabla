<script setup lang="ts">
// One finished turn's tool activity, folded into a single row (point 1 of the
// maintainer's request: "al siguiente turno, que la tool desaparezca").
//
// The chips themselves are not redesigned: expanding this row reveals exactly
// the markup the live turn already shows (`ToolChips`), so the collapsed row is
// a disclosure, not a second way to render a tool call.
//
// The row is never silent about failures: the per-name breakdown disappears
// behind the disclosure, but the failure count stays on the collapsed line in
// the destructive colour. A tool that failed must not hide behind a summary.
import { computed, ref } from "vue";
import { ChevronRightIcon } from "@lucide/vue";
import ToolChips from "@/components/chat/ToolChips.vue";
import { t } from "@/lib/i18n";
import type { ToolEntry } from "@/lib/session-store";

const props = defineProps<{
  /** The turn these chips belong to; keys the disclosure's DOM id. */
  turnId: string;
  /** The turn's tool calls, oldest first. */
  tools: ToolEntry[];
}>();

/**
 * Open/closed state of this row. Component-local on purpose: it is a transient
 * disclosure of a turn that is already settled, not state the store has to keep
 * for another surface, and a remount (a session switch, a template re-render)
 * landing it closed again is the safe direction.
 */
const expanded = ref(false);

/** DOM id of the disclosure, derived from a store id (`12:3`), which is not a
 * safe id as-is. */
const contentId = computed(
  () => `tools-${props.turnId.replace(/[^a-zA-Z0-9_-]/g, "-")}`,
);

/** How many of this turn's calls ended in error; `0` for a clean turn. */
const failures = computed(
  () => props.tools.filter((tool) => tool.state === "error").length,
);

/**
 * The per-name breakdown (`navigate ×7, eval ×5`).
 *
 * Data, not copy: these are the tool names pi reported, and the multiplication
 * sign is punctuation — neither belongs in a locale file. Names keep the order
 * they were first called in, which is the order the turn did its work.
 */
const breakdown = computed(() => {
  const counts = new Map<string, number>();
  for (const tool of props.tools) {
    counts.set(tool.toolName, (counts.get(tool.toolName) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([name, count]) => `${name} ×${count}`)
    .join(", ");
});

function toggle() {
  expanded.value = !expanded.value;
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
      <span class="shrink-0 font-medium">
        {{ t("chat.tools.summary", { count: tools.length }) }}
      </span>
      <span class="min-w-0 truncate"> · {{ breakdown }}</span>
      <!-- Always on the collapsed line: a failure is never behind a summary. -->
      <span v-if="failures > 0" class="shrink-0 text-destructive">
        · {{ t("chat.tools.failures", { count: failures }) }}
      </span>
    </button>
    <div
      v-show="expanded"
      :id="contentId"
      class="mr-2.5 mb-2 ml-2.5 border-l-2 border-border py-1 pr-1 pl-2.5"
    >
      <ToolChips :tools="tools" />
    </div>
  </div>
</template>
