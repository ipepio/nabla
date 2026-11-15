<script setup lang="ts">
import { computed } from "vue";
// Empty chat (spec S1): one brand image, an optional configurable hero line,
// two inert mode/provider chips and the composer, centred in the panel.
//
// Brand paths belong to `src/lib/brand.ts` (S3, ADR 0009): replacing the file in
// `public/` re-brands the app without touching this component. The image is the
// combined mark + wordmark asset, so the panel never renders two logos.
import { ChevronDownIcon } from "@lucide/vue";
import { useAppearance } from "@/lib/appearance";
import { themedBrand } from "@/lib/brand";
import { HERO } from "@/lib/hero";

// Theme-aware: the combined mark + wordmark asset follows the palette, so the
// empty state never shows a white logo on a light background.
const { isDark } = useAppearance();
const logo = computed(() => themedBrand("logo", isDark.value));

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
    <div class="flex w-full max-w-[640px] flex-col items-center gap-6">
      <div class="flex flex-col items-center gap-3">
        <img
          :src="logo"
          alt="Nabla"
          class="h-28 w-auto object-contain opacity-95"
        />
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
