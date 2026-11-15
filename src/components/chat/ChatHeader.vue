<script setup lang="ts">
// Conversation header (spec S2): title + mode chip on the left, actions on the
// right. The theme control is live (spec S2 of
// `odd/tasks/theme-modes-and-resizable-drawer.md`); the overflow menu is still a
// seam and intentionally calls nothing.
//
// The header carries no theme state of its own: `isDark` is the resolved mode
// the panel owns, and `toggleTheme` asks the panel for the next one (S4: the
// theme is applied by `src/lib/theme.ts` only).
import { MoreHorizontalIcon, PanelRightOpenIcon } from "@lucide/vue";
import ThemeToggle from "@/components/chat/ThemeToggle.vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";

defineProps<{
  /** Workspace/session name. */
  title: string;
  /** Interaction mode label; only "Standard mode" exists today. */
  mode: string;
  /** Right tools drawer visibility, so the control reflects the real state. */
  toolsOpen: boolean;
  /** Is the app currently on the dark palette? Decides the icon and the label. */
  isDark: boolean;
}>();

const emit = defineEmits<{
  (e: "toggleTools"): void;
  (e: "toggleTheme"): void;
}>();
</script>

<template>
  <header class="flex items-center gap-2 border-b border-border px-4 py-2">
    <div class="flex min-w-0 flex-1 items-center gap-2">
      <h1 class="truncate text-sm font-semibold">{{ title }}</h1>
      <Badge variant="outline" class="shrink-0 text-muted-foreground">
        {{ mode }}
      </Badge>
    </div>

    <!-- Tools drawer toggle: collapsed by default and reachable from here and
         from the empty state. Hidden while the drawer is open, because the
         drawer carries its own collapse control (no duplicate affordance in
         the central area). -->
    <Button
      v-if="!toolsOpen"
      variant="ghost"
      size="icon-sm"
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

    <!-- Seam: no session menu yet. -->
    <Button
      variant="ghost"
      size="icon-sm"
      :aria-label="t('chat.moreActions')"
    >
      <MoreHorizontalIcon />
    </Button>
  </header>
</template>
