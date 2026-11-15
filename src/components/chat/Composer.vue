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
  <div class="mx-auto w-full max-w-[640px] rounded-2xl border border-border bg-card/60 shadow-sm">
    <Textarea
      :model-value="modelValue"
      aria-label="Prompt"
      placeholder="Describe what you want to build, / commands, @ files or sessions"
      class="max-h-64 min-h-[96px] resize-none border-0 bg-transparent px-3.5 pt-3 pb-1 text-sm shadow-none focus-visible:ring-0 dark:bg-transparent"
      :disabled="disabled"
      @update:model-value="emit('update:modelValue', String($event))"
      @keydown.enter.exact.prevent="emit('send')"
    />

    <div class="flex items-center gap-1 px-2 pb-2">
      <!-- Seam: attaching files/context is not wired yet. -->
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Add context (not wired yet)"
        :disabled="disabled"
      >
        <PlusIcon />
      </Button>

      <span
        class="inline-flex min-w-0 items-center gap-1 rounded-md px-1.5 py-1 text-xs text-muted-foreground"
      >
        <span class="truncate">Writing in {{ workspaceName }}</span>
        <ChevronDownIcon class="size-3 shrink-0" />
      </span>

      <div class="ms-auto flex shrink-0 items-center gap-1.5">
        <!-- Model selector (S5/S7): options from `list_models`, value from
             settings.json. -->
        <Select
          :model-value="modelId"
          :disabled="modelDisabled"
          @update:model-value="emit('update:modelId', String($event))"
        >
          <SelectTrigger
            class="h-8 w-[150px] border-0 bg-transparent text-xs shadow-none hover:bg-muted dark:bg-transparent dark:hover:bg-muted"
            aria-label="Model"
          >
            <SelectValue placeholder="Select model" />
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
          aria-label="Stop session"
          @click="emit('stop')"
        >
          <SquareIcon />
        </Button>
        <Button
          v-else
          size="icon-sm"
          class="rounded-full"
          aria-label="Send prompt"
          :disabled="!canSend"
          @click="emit('send')"
        >
          <ArrowUpIcon />
        </Button>
      </div>
    </div>
  </div>
</template>
