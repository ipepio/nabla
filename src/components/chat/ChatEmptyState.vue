<script setup lang="ts">
import { computed } from "vue";
// Empty chat (spec S1): one brand image, an optional configurable hero line,
// provider chip and the composer, centred in the panel.
//
// Brand paths belong to `src/lib/brand.ts` (S3, ADR 0009): replacing the file in
// `public/` re-brands the app without touching this component. The image is the
// combined mark + wordmark asset, so the panel never renders two logos.
import { useAppearance } from "@/lib/appearance";
import { themedBrand } from "@/lib/brand";
import { HERO } from "@/lib/hero";

// Theme-aware: separate mark and wordmark assets follow the palette so the
// triangle mark can be scaled, spaced cleanly above the wordmark, and animated.
const { isDark } = useAppearance();
const mark = computed(() => themedBrand("markTrimmed", isDark.value));
const wordmark = computed(() => themedBrand("wordmark", isDark.value));

defineProps<{
  /** Provider id from settings.json, shown in the first chip. */
  provider: string;
  /** Guidance shown when no workspace is selected (the composer is inert then). */
  hint: string | null;
}>();
</script>

<template>
  <div
    class="flex h-full min-h-0 flex-col items-center justify-center overflow-x-hidden overflow-y-auto px-4 py-8 sm:px-6"
  >
    <div class="flex w-full min-w-0 max-w-[640px] flex-col items-center gap-6">
      <div class="flex flex-col items-center gap-3">
        <div
          class="group flex cursor-default select-none flex-col items-center gap-4"
          aria-label="Nabla"
        >
          <img
            :src="mark"
            alt=""
            aria-hidden="true"
            class="h-28 w-auto object-contain opacity-95 transition-all duration-300 ease-out group-hover:-translate-y-1.5 group-hover:scale-110 group-hover:rotate-6 group-hover:opacity-100 group-hover:drop-shadow-[0_10px_20px_rgba(0,0,0,0.22)]"
          />
          <img
            :src="wordmark"
            alt="Nabla"
            class="h-3.5 w-auto object-contain opacity-90"
          />
        </div>
        <!-- Optional copy under the logo; both lines hide when unset. -->
        <p
          v-if="HERO.title"
          class="text-center text-base font-medium text-foreground"
        >
          {{ HERO.title }}
        </p>
        <p
          v-if="HERO.subtitle"
          class="max-w-[52ch] text-center text-sm text-muted-foreground"
        >
          {{ HERO.subtitle }}
        </p>
      </div>

      <!-- The provider chip is deliberately inert: it only reports what
           settings.json already says. The mode chip that used to sit beside it
           carried a single possible value, so it was removed — a control that
           cannot change anything is noise. -->
      <div class="flex flex-wrap items-center justify-center gap-2">
        <span
          class="inline-flex items-center gap-1.5 rounded-full border border-border bg-card px-3 py-1 font-mono text-[11px] text-muted-foreground"
        >
          <span class="size-1.5 rounded-full bg-primary" aria-hidden="true" />
          {{ provider }}
        </span>
      </div>

      <slot name="composer" />

      <p v-if="hint" class="text-center text-xs text-muted-foreground">
        {{ hint }}
      </p>
    </div>
  </div>
</template>
