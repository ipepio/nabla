<script setup lang="ts">
// The control that walks a bounded transcript back through its history.
//
// Both transcripts (`ChatPanel.vue` and `SubagentView.vue`) mount only the newest
// page of turns (see `turn-window.ts`), so this is the way to the rest: a real
// button — keyboard operable, and its visible text is its accessible name — that
// says how many turns are still above it. The label is a catalogue key
// (`chat.transcript.showOlder`), so it follows the interface language like every
// other visible string.
import { computed } from "vue";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/lib/i18n";

const props = defineProps<{
  /** How many older turns are not mounted. */
  count: number;
}>();

const emit = defineEmits<{ (e: "reveal"): void }>();

const { t } = useI18n();

const label = computed(() =>
  t("chat.transcript.showOlder", { count: props.count }),
);
</script>

<template>
  <div class="flex justify-center">
    <Button variant="outline" size="xs" @click="emit('reveal')">
      {{ label }}
    </Button>
  </div>
</template>
