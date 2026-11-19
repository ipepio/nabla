<script setup lang="ts">
// The autocomplete list of the composer (S3 `/` commands and S4 `@` mentions of
// `odd/tasks/composer-inputs.md`).
//
// Presentation only: it paints the rows the wiring in `Composer.vue` computed,
// marks the highlighted one and emits which row the human clicked. It never
// filters, never fetches and never decides what a pick means — that is one rule
// per feature, and it lives with the feature, not with the list.
//
// Keyboard: the arrows, Enter, Tab and Escape are handled by the textarea (the
// element that has the focus), so this component only reports clicks. A click is
// answered on `mousedown` with the default prevented, so the textarea never loses
// focus and the caret stays where the completion must land.
import type { SuggestionItem } from "@/components/chat/composer-completion";

defineProps<{
  /** What the list is offering, announced as its accessible name. */
  title: string;
  /** The rows, already filtered and ordered by the caller. */
  items: SuggestionItem[];
  /** Index of the highlighted row. */
  activeIndex: number;
}>();

const emit = defineEmits<{
  (e: "select", index: number): void;
  (e: "highlight", index: number): void;
}>();

/**
 * Keeps the highlighted row visible while the arrows walk past the visible
 * rows. A function ref runs after every patch, and `block: "nearest"` does
 * nothing when the row is already in view, so this needs no watcher: the list
 * scrolls only when the highlight leaves it.
 */
function keepInView(element: unknown, index: number, activeIndex: number): void {
  if (index !== activeIndex) return;
  if (element instanceof HTMLElement) {
    element.scrollIntoView({ block: "nearest" });
  }
}
</script>

<template>
  <!-- Above the card, out of the layout flow: opening a list must never move the
       text the human is looking at. -->
  <div
    class="absolute inset-x-2 bottom-full z-20 mb-1 overflow-hidden rounded-xl border border-border bg-popover shadow-md"
  >
    <p class="px-3 pt-2 pb-1 text-[11px] font-medium text-muted-foreground">
      {{ title }}
    </p>
    <ul
      role="listbox"
      :aria-label="title"
      class="max-h-56 overflow-y-auto p-1"
    >
      <li
        v-for="(item, index) in items"
        :key="item.key"
        :ref="(element) => keepInView(element, index, activeIndex)"
        role="option"
        :aria-selected="index === activeIndex"
        :class="[
          'flex min-w-0 cursor-pointer items-baseline gap-2 rounded-md px-2 py-1.5 text-xs',
          index === activeIndex
            ? 'bg-accent text-accent-foreground'
            : 'text-foreground',
        ]"
        @mousedown.prevent="emit('select', index)"
        @mouseenter="emit('highlight', index)"
      >
        <span class="shrink-0 font-mono">{{ item.label }}</span>
        <span v-if="item.hint" class="min-w-0 flex-1 truncate text-muted-foreground">
          {{ item.hint }}
        </span>
        <span v-else class="flex-1" />
        <span
          v-if="item.badge"
          class="shrink-0 text-[10px] tracking-wide text-muted-foreground uppercase"
        >
          {{ item.badge }}
        </span>
      </li>
    </ul>
  </div>
</template>
