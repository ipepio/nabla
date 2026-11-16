<script setup lang="ts">
// One fenced code block of an assistant turn: a bordered surface with a header
// row (language tag, or a neutral label) and a copy button.
//
// The source arrives already extracted (`code-segments.ts`), so copying is exact:
// no fence markers, no language line, and nothing the view added. While a stream
// has not closed its fence yet the block still renders and still copies — what
// exists is what the human sees and gets.
import { onUnmounted, ref } from "vue";
import { CheckIcon, CopyIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";

const props = defineProps<{
  /** The fence's language tag; `""` when it carried none. */
  language: string;
  /** Exact block source, without fences or language line. */
  source: string;
}>();

const copied = ref(false);
let resetHandle: number | null = null;

async function copySource() {
  try {
    await navigator.clipboard.writeText(props.source);
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

onUnmounted(() => {
  if (resetHandle !== null) window.clearTimeout(resetHandle);
  resetHandle = null;
});
</script>

<template>
  <div class="overflow-hidden rounded-md border border-border bg-muted/30">
    <div class="flex items-center justify-between gap-2 border-b border-border px-2.5 py-1">
      <span
        class="truncate text-[10px] font-medium tracking-wider text-muted-foreground uppercase"
      >
        {{ language === "" ? t("chat.code.label") : language }}
      </span>
      <Button
        variant="ghost"
        size="icon-xs"
        class="-mr-1 shrink-0"
        :aria-label="t('chat.code.copy')"
        @click="copySource"
      >
        <CheckIcon v-if="copied" />
        <CopyIcon v-else />
      </Button>
    </div>
    <pre
      class="overflow-x-auto p-2.5 font-mono text-xs leading-relaxed text-foreground"
    ><code>{{ source }}</code></pre>
  </div>
</template>
