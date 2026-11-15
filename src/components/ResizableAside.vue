<script setup lang="ts">
// Resizable right-hand panel (spec S3, "echo en falta que se pueda redimensionar
// la parte del workspace de la derecha").
//
// OWNERSHIP (spec S4 — one owner per piece of state): this component owns the
// panel width. The parent keeps only whether the panel is open, so the width has
// exactly one home and no duplicated `ref` gets out of sync.
//
// Width is SESSION state on purpose: nothing is persisted here. Remembering the
// width across launches would need a `settings.json` key (task T4 follow-up),
// which is out of scope for this unit.
import type { HTMLAttributes } from "vue";
import { onBeforeUnmount, ref } from "vue";

const DEFAULT_WIDTH = 280;
const MIN_WIDTH = 240;
const MAX_WIDTH = 560;
// Never let the panel eat the chat: at most 60% of the window at drag time.
const MAX_VIEWPORT_RATIO = 0.6;
const KEYBOARD_STEP = 16;

const props = withDefaults(
  defineProps<{
    ariaLabel?: string;
    class?: HTMLAttributes["class"];
  }>(),
  {
    ariaLabel: "Tools",
  },
);

const asideEl = ref<HTMLElement | null>(null);
const width = ref(DEFAULT_WIDTH);
const isDragging = ref(false);

/**
 * Clamp a candidate width. `maxByViewport` covers the "at most 60% of the
 * window" rule while still never falling below the hard minimum on tiny windows.
 */
function clampWidth(candidate: number): number {
  const maxByViewport = Math.floor(window.innerWidth * MAX_VIEWPORT_RATIO);
  const max = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, maxByViewport));
  return Math.round(Math.min(Math.max(candidate, MIN_WIDTH), max));
}

/** The aside is right-aligned: its own rect right edge is the reference axis. */
function rightEdge(): number {
  return asideEl.value?.getBoundingClientRect().right ?? window.innerWidth;
}

function resetWidth(): void {
  width.value = clampWidth(DEFAULT_WIDTH);
}

function onPointerDown(event: PointerEvent): void {
  if (event.button !== 0) return;
  const handle = event.currentTarget as HTMLElement;
  isDragging.value = true;
  handle.setPointerCapture(event.pointerId);
  // Guard against the drag turning into a text selection across the app.
  document.body.style.userSelect = "none";
  event.preventDefault();
}

function onPointerMove(event: PointerEvent): void {
  if (!isDragging.value) return;
  width.value = clampWidth(rightEdge() - event.clientX);
}

function endDrag(event: PointerEvent): void {
  if (!isDragging.value) return;
  isDragging.value = false;
  const handle = event.currentTarget as HTMLElement;
  if (handle.hasPointerCapture(event.pointerId)) {
    handle.releasePointerCapture(event.pointerId);
  }
  document.body.style.userSelect = "";
}

// The panel sits on the right, so widening means moving the edge leftwards.
function onKeyDown(event: KeyboardEvent): void {
  let candidate: number;
  if (event.key === "ArrowLeft") {
    candidate = width.value + KEYBOARD_STEP;
  } else if (event.key === "ArrowRight") {
    candidate = width.value - KEYBOARD_STEP;
  } else if (event.key === "Home") {
    candidate = DEFAULT_WIDTH;
  } else {
    return;
  }
  event.preventDefault();
  width.value = clampWidth(candidate);
}

onBeforeUnmount(() => {
  // The drag can end without a pointerup (unmount mid-gesture, lost capture).
  document.body.style.userSelect = "";
});
</script>

<template>
  <aside
    ref="asideEl"
    :aria-label="props.ariaLabel"
    :class="props.class"
    class="relative shrink-0 flex-col border-l bg-sidebar"
    :style="{ width: `${width}px` }"
  >
    <!--
      Drag handle: a 6px strip straddling the left border. Invisible at rest
      (the border already reads as the seam) and tinted on hover / focus / drag.
    -->
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize tools panel"
      :aria-valuenow="width"
      :aria-valuemin="MIN_WIDTH"
      :aria-valuemax="MAX_WIDTH"
      tabindex="0"
      class="absolute inset-y-0 left-0 z-10 w-1.5 -translate-x-1/2 cursor-col-resize touch-none outline-none transition-colors focus-visible:bg-border"
      :class="isDragging ? 'bg-border' : 'hover:bg-border'"
      @pointerdown="onPointerDown"
      @pointermove="onPointerMove"
      @pointerup="endDrag"
      @pointercancel="endDrag"
      @lostpointercapture="endDrag"
      @keydown="onKeyDown"
      @dblclick="resetWidth"
    />
    <slot />
  </aside>
</template>
