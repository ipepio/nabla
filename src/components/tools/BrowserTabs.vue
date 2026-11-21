<script setup lang="ts">
// The Browser pane's tab strip: the page the human drives, plus one tab per popup
// that page opened (`odd/tasks/drawer-responsiveness-oauth.md`, S3/L3).
//
// PRESENTATION ONLY. The model — one tab per view, one drawn tab, a close that
// returns to the opener, never a closable page tab — lives in `browser-tabs.ts` and
// is pinned by `tests/browser-tabs.test.ts`. This component draws the items it is
// given and emits the two intents a strip has: draw that tab, close that tab. It
// owns no state, so it cannot disagree with the pane that places the webviews.
//
// WHY A POPUP HAS A CLOSE BUTTON AT ALL: macOS never tells the app that a page
// called `window.close()` (wry has no `webViewDidClose:`), so the end of an OAuth
// popup has to be the human's. This button is that end, and closing returns to the
// page that started the flow (`docs/ai/browser-popups.md`).
import { XIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";
import type { BrowserTabStripItem } from "./browser-tabs";

defineProps<{
  /** The tabs in draw order, already resolved to what a strip shows. */
  items: BrowserTabStripItem[];
}>();

const emit = defineEmits<{
  /** Draw this tab: the pane places its view on the body rect and shows it. */
  select: [id: string];
  /** Close this popup: the pane closes its view and returns to the opener tab. */
  close: [id: string];
}>();
</script>

<template>
  <div
    class="flex shrink-0 items-center gap-1 overflow-x-auto border-b px-1.5 py-1"
    role="tablist"
  >
    <div
      v-for="item in items"
      :key="item.id"
      class="flex shrink-0 items-center overflow-hidden rounded-md border"
      :class="item.active ? 'bg-accent' : 'bg-transparent'"
    >
      <button
        type="button"
        role="tab"
        class="max-w-40 truncate px-2 py-0.5 text-xs"
        :class="item.active ? 'text-foreground' : 'text-muted-foreground'"
        :aria-selected="item.active"
        :title="item.label"
        @click="emit('select', item.id)"
      >
        {{ item.label }}
      </button>
      <Button
        v-if="item.closable"
        variant="ghost"
        size="icon-xs"
        class="shrink-0"
        :aria-label="t('tools.tabs.close')"
        :title="t('tools.tabs.close')"
        @click="emit('close', item.id)"
      >
        <XIcon />
      </Button>
    </div>
  </div>
</template>
