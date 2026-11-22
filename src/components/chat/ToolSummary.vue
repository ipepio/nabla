<script setup lang="ts">
// One human turn's work, folded into a single row (S1 of
// `chat-turn-groups.md`: "debería de estar todo junto y desplegarse. y que fuera
// un desplegable por turno").
//
// The row is the disclosure, not a second way to render anything: opening it
// reveals what the turn passed through — each step's reasoning block, its text
// and its tool chips — through the default slot. One turn, one box, however many
// steps pi took.
//
// The row is never silent about failures: the per-name breakdown disappears
// behind the disclosure, but the failure count stays on the collapsed line in
// the destructive colour (S4). A tool that failed must not hide behind a summary.
import { computed, ref } from "vue";
import { ChevronRightIcon } from "@lucide/vue";
import { t } from "@/lib/i18n";
import type { ToolEntry } from "@/lib/session-store";

const props = defineProps<{
  /** The turn these chips belong to; keys the disclosure's DOM id. */
  turnId: string;
  /** What the collapsed row says, e.g. "3 steps". */
  label: string;
  /** Every chip of the turn, for the breakdown and the failure count. */
  tools: ToolEntry[];
  /**
   * Open the disclosure on mount. Off for a conversation (S1: collapsed by
   * default); on for a read-only surface where the work *is* what the human came
   * to watch and there is no live tail to follow.
   */
  initialExpanded?: boolean;
}>();

/**
 * Open/closed state of this row. Component-local on purpose: it is a transient
 * disclosure of a turn that is already settled, not state the store has to keep
 * for another surface, and a remount (a session switch, a template re-render)
 * landing it closed again is the safe direction (S1: collapsed by default).
 */
const expanded = ref(props.initialExpanded === true);

/** DOM id of the disclosure, derived from a store id (`12:3`), which is not a
 * safe id as-is. */
const contentId = computed(
  () => `turn-${props.turnId.replace(/[^a-zA-Z0-9_-]/g, "-")}`,
);

/** How many of this turn's calls ended in error; `0` for a clean turn. */
const failures = computed(
  () => props.tools.filter((tool) => tool.state === "error").length,
);

/** How many of this turn's calls finished cleanly. */
const completed = computed(
  () => props.tools.filter((tool) => tool.state === "done").length,
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
  <div class="overflow-hidden rounded-xl border border-border bg-card">
    <button
      type="button"
      class="flex w-full items-center gap-2.5 px-3.5 py-2.5 text-left transition-colors hover:bg-secondary/70"
      :aria-expanded="expanded"
      :aria-controls="contentId"
      @click="toggle"
    >
      <ChevronRightIcon
        class="size-3.5 shrink-0 text-muted-foreground transition-transform"
        :class="{ 'rotate-90': expanded }"
        aria-hidden="true"
      />
      <span class="flex min-w-0 flex-1 flex-col gap-0.5">
        <span class="truncate text-xs font-semibold text-foreground">{{ label }}</span>
        <span
          v-if="breakdown !== ''"
          class="truncate font-mono text-[10px] text-muted-foreground"
        >
          {{ breakdown }}
        </span>
      </span>
      <!-- Always on the collapsed line: a failure is never behind a summary. -->
      <span
        v-if="failures > 0"
        class="shrink-0 rounded border border-destructive/25 bg-destructive/10 px-1.5 py-0.5 font-mono text-[10px] font-semibold text-destructive"
      >
        {{ t("chat.tools.failures", { count: failures }) }}
      </span>
      <span
        v-if="completed > 0"
        class="shrink-0 rounded border border-primary/25 bg-primary/10 px-1.5 py-0.5 font-mono text-[10px] font-semibold text-primary"
      >
        {{ completed }} OK
      </span>
    </button>
    <div
      v-show="expanded"
      :id="contentId"
      class="nabla-disclosure border-t border-sidebar-border px-3.5 py-3"
    >
      <slot />
    </div>
  </div>
</template>
