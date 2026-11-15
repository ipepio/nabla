<script setup lang="ts">
// Empty chat (spec S1): the brand mark, two inert mode/provider chips and the
// composer, centred in the panel — no conversation header yet.
//
// Brand paths belong to `src/lib/brand.ts` (S3, ADR 0009): replacing the file in
// `public/` re-brands the app without touching this component.
import { ChevronDownIcon } from "@lucide/vue";
import { brandAsset } from "@/lib/brand";

const mark = brandAsset("mark");
const wordmark = brandAsset("wordmark");

defineProps<{
  /** Provider id from settings.json, shown in the first chip. */
  provider: string;
  /** Guidance shown when no workspace is selected (the composer is inert then). */
  hint: string | null;
}>();
</script>

<template>
  <div
    class="flex h-full min-h-0 flex-col items-center justify-center overflow-y-auto px-6 py-10"
  >
    <div class="flex w-full max-w-[720px] flex-col items-center gap-6">
      <div class="flex flex-col items-center gap-2">
        <img
          :src="mark"
          alt="Nabla"
          class="size-16 object-contain opacity-95"
        />
        <img :src="wordmark" alt="" class="h-3 object-contain opacity-70" />
      </div>

      <!-- Both chips are deliberately inert: the mode is a seam, the provider
           only reports what settings.json already says. -->
      <div class="flex flex-wrap items-center justify-center gap-2">
        <span
          class="inline-flex items-center gap-1 rounded-full border border-border bg-card/60 px-2.5 py-1 text-xs text-muted-foreground"
        >
          {{ provider }}
        </span>
        <span
          class="inline-flex items-center gap-1 rounded-full border border-border bg-card/60 px-2.5 py-1 text-xs text-muted-foreground"
        >
          Standard mode
          <ChevronDownIcon class="size-3" />
        </span>
      </div>

      <slot name="composer" />

      <p v-if="hint" class="text-center text-xs text-muted-foreground">
        {{ hint }}
      </p>
    </div>
  </div>
</template>
