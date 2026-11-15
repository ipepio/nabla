<script setup lang="ts">
// Composer card (spec S2): auto-growing prompt, inert "+" and scope chip, the
// real model selector and the circular send/stop control.
//
// Presentation only: every action is emitted, so ChatPanel keeps the single
// `sendPrompt` / `stopSession` path and this card never touches the contracts
// seam. Colours come from semantic tokens only (S4) — no hex here.
import { ArrowUpIcon, ChevronDownIcon, PlusIcon, SquareIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { t } from "@/lib/i18n";
import { COMPOSER_MAX_WIDTH, COMPOSER_MIN_WIDTH } from "@/lib/layout";
import type { ModelInfo } from "@/contracts/types";

defineProps<{
  /** Prompt text (v-model). */
  modelValue: string;
  /** Options come from `list_models`; the UI never hardcodes them (S7). */
  models: ModelInfo[];
  /** Persisted model id from settings.json, `""` until settings load. */
  modelId: string;
  /** Workspace the turn is written into, for the scope chip. */
  workspaceName: string;
  /** A send is in flight: the primary control becomes stop. */
  streaming: boolean;
  /** Nothing can be sent yet (no workspace selected). */
  disabled: boolean;
  /** The prompt is non-empty and no send is in flight. */
  canSend: boolean;
  /** Model selection is unavailable (saving, or no options reported). */
  modelDisabled: boolean;
  /** A session exists, so stop is meaningful. */
  canStop: boolean;
}>();

const emit = defineEmits<{
  (e: "update:modelValue", value: string): void;
  (e: "update:modelId", value: string): void;
  (e: "send"): void;
  (e: "stop"): void;
}>();
</script>

<template>
  <!-- The composer owns its own max width (single source of truth): the empty
       state and the transcript composer must stay the same size, and parents no
       longer pass a conflicting cap. -->
  <!-- Bounds come from `src/lib/layout.ts`: the box shrinks with its column and
       stops at half of its normal width, which is also the chat's floor. -->
  <div
    class="mx-auto w-full min-w-0 rounded-2xl border border-border bg-card/60 shadow-sm"
    :style="{ maxWidth: `${COMPOSER_MAX_WIDTH}px`, minWidth: `${COMPOSER_MIN_WIDTH}px` }"
  >
    <Textarea
      :model-value="modelValue"
      :aria-label="t('chat.composer.prompt')"
      :placeholder="t('chat.composer.placeholder')"
      class="max-h-64 min-h-[96px] resize-none border-0 bg-transparent px-3.5 pt-3 pb-1 text-sm shadow-none focus-visible:ring-0 dark:bg-transparent"
      :disabled="disabled"
      @update:model-value="emit('update:modelValue', String($event))"
      @keydown.enter.exact.prevent="emit('send')"
    />

    <div class="flex min-w-0 items-center gap-1 px-2 pb-2">
      <!-- Seam: attaching files/context is not wired yet. -->
      <Button
        variant="ghost"
        size="icon-sm"
        :aria-label="t('chat.composer.addContext')"
        :disabled="disabled"
      >
        <PlusIcon />
      </Button>

      <span
        class="hidden min-w-0 items-center gap-1 rounded-md px-1.5 py-1 text-xs text-muted-foreground sm:inline-flex"
      >
        <span class="truncate">
          {{ t("chat.composer.writingIn", { workspace: workspaceName }) }}
        </span>
        <ChevronDownIcon class="size-3 shrink-0" />
      </span>

      <div class="ms-auto flex min-w-0 shrink items-center gap-1.5">
        <!-- Model selector (S5/S7): options from `list_models`, value from
             settings.json. -->
        <Select
          :model-value="modelId"
          :disabled="modelDisabled"
          @update:model-value="emit('update:modelId', String($event))"
        >
          <SelectTrigger
            class="h-8 w-[118px] min-w-0 border-0 bg-transparent text-xs shadow-none hover:bg-muted sm:w-[150px] dark:bg-transparent dark:hover:bg-muted"
            :aria-label="t('chat.composer.model')"
          >
            <SelectValue :placeholder="t('chat.composer.selectModel')" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem v-for="model in models" :key="model.id" :value="model.id">
              {{ model.name }}
            </SelectItem>
          </SelectContent>
        </Select>

        <Button
          v-if="streaming && canStop"
          variant="outline"
          size="icon-sm"
          class="rounded-full"
          :aria-label="t('chat.composer.stop')"
          @click="emit('stop')"
        >
          <SquareIcon />
        </Button>
        <Button
          v-else
          size="icon-sm"
          class="rounded-full"
          :aria-label="t('chat.composer.send')"
          :disabled="!canSend"
          @click="emit('send')"
        >
          <ArrowUpIcon />
        </Button>
      </div>
    </div>
  </div>
</template>
