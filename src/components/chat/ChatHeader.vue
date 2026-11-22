<script setup lang="ts">
// Conversation header (spec S2): title + mode chip on the left, actions on the
// right. The theme control is live (spec S2 of
// `odd/tasks/theme-modes-and-resizable-drawer.md`); the overflow menu is still a
// seam and intentionally calls nothing.
//
// The header carries no theme state of its own: `isDark` is the resolved mode
// the panel owns, and `toggleTheme` asks the panel for the next one (S4: the
// theme is applied by `src/lib/theme.ts` only).
import { PanelRightOpenIcon, SearchIcon } from "@lucide/vue";
import ThemeToggle from "@/components/chat/ThemeToggle.vue";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";

const isMac =
  typeof navigator !== "undefined" &&
  /mac/i.test(navigator.platform || navigator.userAgent);
const searchShortcut = isMac ? "⌘ K" : "Ctrl+K";

defineProps<{
  /** Workspace/session name. */
  title: string;
  /** Active session title, shown as the second breadcrumb segment when set. */
  sessionTitle?: string | null;
  /** Whether the agent is currently running a turn. */
  working?: boolean;
  /** Right tools drawer visibility, so the control reflects the real state. */
  toolsOpen: boolean;
  /** Is the app currently on the dark palette? Decides the icon and the label. */
  isDark: boolean;
}>();

const emit = defineEmits<{
  (e: "toggleTools"): void;
  (e: "toggleTheme"): void;
  (e: "openSearch"): void;
}>();
</script>

<template>
  <header
    class="flex h-13 shrink-0 items-center gap-2.5 border-b border-sidebar-border bg-background px-5"
  >
    <div class="flex min-w-0 flex-1 items-center gap-2 text-xs">
      <span
        v-if="sessionTitle"
        class="shrink-0 truncate text-muted-foreground"
      >
        {{ title }}
      </span>
      <span
        v-if="sessionTitle"
        class="shrink-0 text-muted-foreground/50"
        aria-hidden="true"
      >
        /
      </span>
      <h1 class="truncate text-[13px] font-semibold text-foreground">
        {{ sessionTitle || title }}
      </h1>
    </div>

    <button
      type="button"
      class="hidden items-center gap-2 rounded-lg border border-border bg-card px-2.5 py-1 text-[11px] text-muted-foreground transition-colors outline-hidden hover:bg-secondary hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring sm:inline-flex"
      :aria-label="t('palette.title')"
      @click="emit('openSearch')"
    >
      <SearchIcon class="size-3.5 shrink-0" aria-hidden="true" />
      <span>{{ t("palette.trigger") }}</span>
      <kbd class="font-mono text-[10px] text-muted-foreground/80">
        {{ searchShortcut }}
      </kbd>
    </button>

    <div
      v-if="working"
      class="inline-flex shrink-0 items-center gap-1.5 rounded-full border border-primary/25 bg-primary/10 px-2.5 py-1 text-[10px] font-semibold text-primary"
      role="status"
    >
      <span
        class="nabla-status-pulse size-1.5 rounded-full bg-primary"
        aria-hidden="true"
      />
      <span>{{ t("session.working") }}</span>
    </div>

    <!-- Tools drawer toggle: collapsed by default and reachable from here and
         from the empty state. Hidden while the drawer is open, because the
         drawer carries its own collapse control (no duplicate affordance in
         the central area). -->
    <Button
      v-if="!toolsOpen"
      variant="ghost"
      size="icon-sm"
      class="text-muted-foreground hover:bg-accent hover:text-foreground"
      :aria-label="t('chat.expandTools')"
      :aria-pressed="toolsOpen"
      @click="emit('toggleTools')"
    >
      <PanelRightOpenIcon />
    </Button>

    <!-- Theme switch: the icon names the *target* palette (sun = light), and
         the panel persists the new mode in settings.json. Shared with the
         empty-state cluster so the control never depends on having a turn. -->
    <ThemeToggle :is-dark="isDark" @toggle="emit('toggleTheme')" />
  </header>
</template>
