<script setup lang="ts">
/**
 * Global custom right-click context menu for Nabla.
 *
 * Replaces the default WebView "Inspect Element / Reload" menu with
 * context-aware desktop actions:
 * - On inputs/textareas: Cut, Copy, Paste, Select all.
 * - On selected text anywhere: Copy, Select all.
 * - On the general surface: New session, Search, New document, Toggle tools, Settings.
 *
 * In dev builds (`import.meta.env.DEV`), holding Shift while right-clicking
 * still passes through to the browser's native DevTools menu.
 */
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import {
  CirclePlusIcon,
  ClipboardPasteIcon,
  CopyIcon,
  FilePlus2Icon,
  PanelRightIcon,
  ScissorsIcon,
  SearchIcon,
  SettingsIcon,
  TextSelectIcon,
} from "@lucide/vue";
import { t } from "@/lib/i18n";

const emit = defineEmits<{
  (e: "newSession"): void;
  (e: "openSearch"): void;
  (e: "newDocument"): void;
  (e: "toggleTools"): void;
  (e: "openSettings"): void;
}>();

const visible = ref(false);
const posX = ref(0);
const posY = ref(0);
const hasSelection = ref(false);
const selectedText = ref("");
const activeInput = ref<HTMLInputElement | HTMLTextAreaElement | null>(null);

const isMac =
  typeof navigator !== "undefined" &&
  /mac/i.test(navigator.platform || navigator.userAgent);
const mod = isMac ? "⌘" : "Ctrl+";

const isEditable = computed(() => activeInput.value !== null);

function close(): void {
  visible.value = false;
  activeInput.value = null;
}

function findEditable(target: EventTarget | null): HTMLInputElement | HTMLTextAreaElement | null {
  if (!(target instanceof HTMLElement)) return null;
  if (target instanceof HTMLTextAreaElement && !target.readOnly && !target.disabled) {
    return target;
  }
  if (
    target instanceof HTMLInputElement &&
    !target.readOnly &&
    !target.disabled &&
    !["checkbox", "radio", "button", "submit", "range", "color"].includes(target.type)
  ) {
    return target;
  }
  return null;
}

function handleContextMenu(event: MouseEvent): void {
  // Respect component-specific context menus (e.g. session/subagent rows).
  if (event.defaultPrevented) return;
  // Allow Shift+Right-Click in dev builds to reach WebView DevTools.
  if (import.meta.env.DEV && event.shiftKey) return;
  event.preventDefault();

  const editable = findEditable(event.target);
  activeInput.value = editable;

  let selection = "";
  if (editable) {
    const start = editable.selectionStart ?? 0;
    const end = editable.selectionEnd ?? 0;
    selection = editable.value.slice(start, end);
  } else {
    selection = window.getSelection()?.toString() ?? "";
  }
  selectedText.value = selection;
  hasSelection.value = selection.length > 0;

  const menuW = 224;
  const menuH = editable ? 280 : 210;
  posX.value = Math.min(event.clientX, Math.max(8, window.innerWidth - menuW - 8));
  posY.value = Math.min(event.clientY, Math.max(8, window.innerHeight - menuH - 8));
  visible.value = true;
}

function handleKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape" && visible.value) {
    close();
  }
}

async function doCopy(): Promise<void> {
  const text = selectedText.value;
  close();
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    document.execCommand("copy");
  }
}

async function doCut(): Promise<void> {
  const el = activeInput.value;
  const text = selectedText.value;
  close();
  if (!el || !text) return;
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // ignore clipboard error
  }
  const start = el.selectionStart ?? 0;
  const end = el.selectionEnd ?? 0;
  el.focus();
  el.setRangeText("", start, end, "end");
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

async function doPaste(): Promise<void> {
  const el = activeInput.value;
  close();
  if (!el) return;
  try {
    const clip = await navigator.clipboard.readText();
    if (!clip) return;
    const start = el.selectionStart ?? el.value.length;
    const end = el.selectionEnd ?? el.value.length;
    el.focus();
    el.setRangeText(clip, start, end, "end");
    el.dispatchEvent(new Event("input", { bubbles: true }));
  } catch {
    el.focus();
    document.execCommand("paste");
  }
}

function doSelectAll(): void {
  const el = activeInput.value;
  close();
  if (el) {
    el.focus();
    el.select();
    return;
  }
  document.execCommand("selectAll");
}

function runAction(fn: () => void): void {
  close();
  fn();
}

onMounted(() => {
  window.addEventListener("contextmenu", handleContextMenu);
  window.addEventListener("pointerdown", close);
  window.addEventListener("keydown", handleKeydown);
  window.addEventListener("blur", close);
});

onBeforeUnmount(() => {
  window.removeEventListener("contextmenu", handleContextMenu);
  window.removeEventListener("pointerdown", close);
  window.removeEventListener("keydown", handleKeydown);
  window.removeEventListener("blur", close);
});
</script>

<template>
  <Teleport to="body">
    <div
      v-if="visible"
      role="menu"
      class="fixed z-50 w-56 overflow-hidden rounded-xl border border-border bg-popover p-1 text-popover-foreground shadow-xl"
      :style="{ left: `${posX}px`, top: `${posY}px` }"
      @pointerdown.stop
      @contextmenu.prevent
    >
      <template v-if="isEditable || hasSelection">
        <button
          v-if="isEditable && hasSelection"
          type="button"
          role="menuitem"
          class="flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
          @click="doCut"
        >
          <span class="flex items-center gap-2">
            <ScissorsIcon class="size-3.5 text-muted-foreground" />
            {{ t("context.cut") }}
          </span>
          <kbd class="font-mono text-[10px] text-muted-foreground">{{ mod }}X</kbd>
        </button>

        <button
          v-if="hasSelection"
          type="button"
          role="menuitem"
          class="flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
          @click="doCopy"
        >
          <span class="flex items-center gap-2">
            <CopyIcon class="size-3.5 text-muted-foreground" />
            {{ t("context.copy") }}
          </span>
          <kbd class="font-mono text-[10px] text-muted-foreground">{{ mod }}C</kbd>
        </button>

        <button
          v-if="isEditable"
          type="button"
          role="menuitem"
          class="flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
          @click="doPaste"
        >
          <span class="flex items-center gap-2">
            <ClipboardPasteIcon class="size-3.5 text-muted-foreground" />
            {{ t("context.paste") }}
          </span>
          <kbd class="font-mono text-[10px] text-muted-foreground">{{ mod }}V</kbd>
        </button>

        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
          @click="doSelectAll"
        >
          <span class="flex items-center gap-2">
            <TextSelectIcon class="size-3.5 text-muted-foreground" />
            {{ t("context.selectAll") }}
          </span>
          <kbd class="font-mono text-[10px] text-muted-foreground">{{ mod }}A</kbd>
        </button>

        <div class="my-1 h-px bg-border" role="separator" />
      </template>

      <button
        type="button"
        role="menuitem"
        class="flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
        @click="runAction(() => emit('newSession'))"
      >
        <span class="flex items-center gap-2">
          <CirclePlusIcon class="size-3.5 text-muted-foreground" />
          {{ t("palette.action.newSession") }}
        </span>
        <kbd class="font-mono text-[10px] text-muted-foreground">{{ mod }}N</kbd>
      </button>

      <button
        type="button"
        role="menuitem"
        class="flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
        @click="runAction(() => emit('openSearch'))"
      >
        <span class="flex items-center gap-2">
          <SearchIcon class="size-3.5 text-muted-foreground" />
          {{ t("palette.trigger") }}
        </span>
        <kbd class="font-mono text-[10px] text-muted-foreground">{{ mod }}K</kbd>
      </button>

      <button
        type="button"
        role="menuitem"
        class="flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
        @click="runAction(() => emit('newDocument'))"
      >
        <span class="flex items-center gap-2">
          <FilePlus2Icon class="size-3.5 text-muted-foreground" />
          {{ t("palette.action.newDocument") }}
        </span>
      </button>

      <button
        type="button"
        role="menuitem"
        class="flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
        @click="runAction(() => emit('toggleTools'))"
      >
        <span class="flex items-center gap-2">
          <PanelRightIcon class="size-3.5 text-muted-foreground" />
          {{ t("palette.action.toggleTools") }}
        </span>
        <kbd class="font-mono text-[10px] text-muted-foreground">{{ mod }}.</kbd>
      </button>

      <button
        type="button"
        role="menuitem"
        class="flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
        @click="runAction(() => emit('openSettings'))"
      >
        <span class="flex items-center gap-2">
          <SettingsIcon class="size-3.5 text-muted-foreground" />
          {{ t("palette.action.settings") }}
        </span>
        <kbd class="font-mono text-[10px] text-muted-foreground">{{ mod }},</kbd>
      </button>
    </div>
  </Teleport>
</template>
