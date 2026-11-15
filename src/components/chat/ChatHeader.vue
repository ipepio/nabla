<script setup lang="ts">
// Conversation header (spec S2): title + mode chip on the left, actions on the
// right. Both actions are seams — theme switching and the overflow menu are not
// wired yet, so they intentionally call nothing.
import {
  MoonIcon,
  MoreHorizontalIcon,
  PanelRightCloseIcon,
  PanelRightOpenIcon,
  SunIcon,
} from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";

defineProps<{
  /** Workspace/session name. */
  title: string;
  /** Interaction mode label; only "Standard mode" exists today. */
  mode: string;
  /** Right tools drawer visibility, so the control reflects the real state. */
  toolsOpen: boolean;
}>();

const emit = defineEmits<{ (e: "toggleTools"): void }>();
</script>

<template>
  <header class="flex items-center gap-2 border-b border-border px-4 py-2">
    <div class="flex min-w-0 flex-1 items-center gap-2">
      <h1 class="truncate text-sm font-semibold">{{ title }}</h1>
      <Badge variant="outline" class="shrink-0 text-muted-foreground">
        {{ mode }}
      </Badge>
    </div>

    <!-- Tools drawer toggle: collapsed by default, reachable from here and
         from the empty state so it never becomes unreachable. -->
    <Button
      variant="ghost"
      size="icon-sm"
      :aria-label="toolsOpen ? 'Collapse tools panel' : 'Expand tools panel'"
      :aria-pressed="toolsOpen"
      @click="emit('toggleTools')"
    >
      <PanelRightCloseIcon v-if="toolsOpen" />
      <PanelRightOpenIcon v-else />
    </Button>

    <!-- Seam: no theme state is persisted from the UI yet (settings.theme is
         written by the settings file, not by this button). -->
    <Button
      variant="ghost"
      size="icon-sm"
      aria-label="Toggle theme (not wired yet)"
    >
      <SunIcon class="hidden dark:block" />
      <MoonIcon class="dark:hidden" />
    </Button>

    <!-- Seam: no session menu yet. -->
    <Button variant="ghost" size="icon-sm" aria-label="More actions (not wired yet)">
      <MoreHorizontalIcon />
    </Button>
  </header>
</template>
