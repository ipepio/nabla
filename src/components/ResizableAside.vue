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
import { onBeforeUnmount, onMounted, ref } from "vue";
import { CHAT_MIN_WIDTH } from "@/lib/layout";

/**
 * The width of a fresh panel: half the window.
 *
 * A tool pane is a workspace, not a sidebar. At the old 280px a browser or a
 * terminal was unusable — the human's first act was always to drag it wider — so
 * the panel now opens at half the window and the human narrows it (or
 * double-clicks back to this) from there. `clampWidth` still bounds the result:
 * the chat keeps its own minimum, and narrow windows shrink the panel with it.
 *
 * The same value serves the two ways a panel appears: the human opening the
 * drawer, and the agent asking for it (`embed-need-view`). A width the human
 * dragged by hand is never overwritten — this is only what a panel starts at,
 * and what a double-click restores.
 */
function defaultWidth(): number {
  if (typeof window === "undefined") return 280;
  return Math.round(window.innerWidth / 2);
}

const MIN_WIDTH = 240;
const KEYBOARD_STEP = 16;

/**
 * Hard floor for the chat column, imported from `src/lib/layout.ts`: the
 * composer's minimum (half of its normal width) plus its wrapper padding. The
 * panel grows freely until the chat is exactly that wide and no further.
 */

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
const width = ref(defaultWidth());
const isDragging = ref(false);

/**
 * The widest this panel may get right now: whatever the chat leaves. There is no
 * fixed cap — dragging is limited only by the chat's minimum, measured on the
 * flex row that owns both siblings.
 */
function maxWidth(): number {
  const available =
    asideEl.value?.parentElement?.clientWidth ?? window.innerWidth;
  return Math.max(MIN_WIDTH, available - CHAT_MIN_WIDTH);
}

/** Clamp a candidate width to [MIN_WIDTH, maxWidth()]. */
function clampWidth(candidate: number): number {
  return Math.round(Math.min(Math.max(candidate, MIN_WIDTH), maxWidth()));
}

/** Width ceiling for the ARIA values, read at render time. */
const maxWidthNow = ref(defaultWidth());

/** Keep the ceiling current and never let a shrink hide the chat. */
function handleViewportResize(): void {
  maxWidthNow.value = maxWidth();
  if (width.value > maxWidthNow.value) width.value = maxWidthNow.value;
}

/** The aside is right-aligned: its own rect right edge is the reference axis. */
function rightEdge(): number {
  return asideEl.value?.getBoundingClientRect().right ?? window.innerWidth;
}

function resetWidth(): void {
  width.value = clampWidth(defaultWidth());
}

function onPointerDown(event: PointerEvent): void {
  if (event.button !== 0) return;
  const handle = event.currentTarget as HTMLElement;
  isDragging.value = true;
  maxWidthNow.value = maxWidth();
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
    candidate = defaultWidth();
  } else {
    return;
  }
  event.preventDefault();
  width.value = clampWidth(candidate);
}

onMounted(() => {
  handleViewportResize();
  window.addEventListener("resize", handleViewportResize);
});

onBeforeUnmount(() => {
  window.removeEventListener("resize", handleViewportResize);
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
      :aria-valuemax="maxWidthNow"
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
