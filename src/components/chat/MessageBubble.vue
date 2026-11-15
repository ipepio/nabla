<script setup lang="ts">
// One transcript entry (spec S2): user turns are right-aligned bubbles with a
// timestamp and a copy control; assistant turns are full-width plain text, so
// streamed deltas read like a document rather than a chat log.
import { ref } from "vue";
import { CheckIcon, CopyIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";

const props = defineProps<{
  role: "user" | "assistant";
  text: string;
  /** `HH:MM`, user turns only. */
  at?: string;
}>();

const copied = ref(false);
let resetHandle: number | null = null;

async function copyText() {
  try {
    await navigator.clipboard.writeText(props.text);
    copied.value = true;
    if (resetHandle !== null) window.clearTimeout(resetHandle);
    resetHandle = window.setTimeout(() => {
      copied.value = false;
      resetHandle = null;
    }, 1500);
  } catch {
    // A denied clipboard write leaves the control in its idle state; there is
    // no toast layer yet to report it.
    copied.value = false;
  }
}
</script>

<template>
  <div v-if="role === 'user'" class="flex flex-col items-end gap-1">
    <div
      class="max-w-[75%] rounded-2xl bg-secondary px-3.5 py-2 text-sm whitespace-pre-wrap text-secondary-foreground"
    >
      {{ text }}
    </div>
    <div class="flex items-center gap-1 text-xs text-muted-foreground">
      <span v-if="at">{{ at }}</span>
      <Button variant="ghost" size="icon-xs" aria-label="Copy message" @click="copyText">
        <CheckIcon v-if="copied" />
        <CopyIcon v-else />
      </Button>
    </div>
  </div>

  <p v-else class="text-sm whitespace-pre-wrap text-foreground">{{ text }}</p>
</template>
