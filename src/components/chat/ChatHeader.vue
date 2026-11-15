<script setup lang="ts">
// Conversation header (spec S2): title + mode chip on the left, actions on the
// right. Both actions are seams — theme switching and the overflow menu are not
// wired yet, so they intentionally call nothing.
import {
  MoonIcon,
  MoreHorizontalIcon,
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

    <!-- Tools drawer toggle: collapsed by default and reachable from here and
         from the empty state. Hidden while the drawer is open, because the
         drawer carries its own collapse control (no duplicate affordance in
         the central area). -->
    <Button
      v-if="!toolsOpen"
      variant="ghost"
      size="icon-sm"
      aria-label="Expand tools panel"
      :aria-pressed="toolsOpen"
      @click="emit('toggleTools')"
    >
      <PanelRightOpenIcon />
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
