<script setup lang="ts">
// Theme switch (spec S2). Shared by the conversation header and the empty-state
// corner cluster so the control is reachable before the first turn too.
//
// Presentation only: the mode lives in ChatPanel (persisted in settings.json),
// so this button never touches the contracts seam.
import { MoonIcon, SunIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";

const props = defineProps<{
  /** Current resolved appearance. */
  isDark: boolean;
}>();

const emit = defineEmits<{ (e: "toggle"): void }>();
</script>

<template>
  <Button
    variant="ghost"
    size="icon-sm"
    :aria-label="
      props.isDark ? t('theme.switchToLight') : t('theme.switchToDark')
    "
    :aria-pressed="props.isDark"
    @click="emit('toggle')"
  >
    <SunIcon v-if="props.isDark" />
    <MoonIcon v-else />
  </Button>
</template>
