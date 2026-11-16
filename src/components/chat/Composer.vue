<script setup lang="ts">
// Composer card (spec S2): auto-growing prompt, inert "+" and scope chip, the
// real model selector and the circular send/stop control.
//
// Presentation only: every action is emitted, so ChatPanel keeps the single
// `sendPrompt` / `stopSession` path and this card never touches the contracts
// seam. Colours come from semantic tokens only (S4) — no hex here.
import { computed } from "vue";
import { ArrowUpIcon, ChevronDownIcon, PlusIcon, SquareIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { t } from "@/lib/i18n";
import { COMPOSER_MAX_WIDTH, COMPOSER_MIN_WIDTH } from "@/lib/layout";
import type { ProviderModels } from "@/contracts/types";

/**
 * A model choice is provider-scoped (S2 of `models-by-provider.md`): the same id
 * can exist under two providers, so picking one fixes both halves at once.
 */
interface ModelChoice {
  provider: string;
  model: string;
}

/**
 * Separator of one select value. Provider ids are `[a-z0-9-]` (the contract of
 * `providers.<id>` in pi's file), so the first separator is always the boundary
 * and a model id may contain anything — `::` included.
 */
const MODEL_VALUE_SEPARATOR = "::";

/** The value one item carries: provider, then model. */
function modelValueOf(provider: string, model: string): string {
  return `${provider}${MODEL_VALUE_SEPARATOR}${model}`;
}

/** The inverse of `modelValueOf`; `null` for anything the surface did not build. */
function modelChoiceOf(value: string): ModelChoice | null {
  const at = value.indexOf(MODEL_VALUE_SEPARATOR);
  if (at <= 0) return null;
  const model = value.slice(at + MODEL_VALUE_SEPARATOR.length);
  if (model === "") return null;
  return { provider: value.slice(0, at), model };
}

const props = defineProps<{
  /** Prompt text (v-model). */
  modelValue: string;
  /**
   * Providers with their models, from `list_provider_models` (S2). The UI never
   * hardcodes a model id: the grouped list is exactly what pi's file holds.
   */
  providerModels: ProviderModels[];
  /** Persisted provider id from settings.json, `""` until settings load. */
  providerId: string;
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
  /** Model selection is unavailable (saving, or no enabled model reported). */
  modelDisabled: boolean;
  /** A session exists, so stop is meaningful. */
  canStop: boolean;
}>();

const emit = defineEmits<{
  (e: "update:modelValue", value: string): void;
  (e: "selectModel", choice: ModelChoice): void;
  (e: "send"): void;
  (e: "stop"): void;
}>();

/**
 * S2: one group per provider that has something to offer, one item per enabled
 * model — `enabled === false` is hidden here, not deleted from the file (L3). A
 * provider with no usable model is not painted as an empty group.
 */
const groups = computed(() =>
  props.providerModels
    .map((entry) => ({
      provider: entry.provider,
      name: entry.name.trim() === "" ? entry.provider : entry.name,
      models: entry.models.filter((model) => model.enabled),
    }))
    .filter((group) => group.models.length > 0),
);

/**
 * The persisted choice resolved against the list that is on screen. The
 * persisted provider wins, because a model id belongs to it; the search only
 * falls back to every group while settings have not named one yet. A model the
 * list does not carry resolves to `null` and is shown as the raw id below.
 */
const selected = computed<{
  provider: string;
  providerName: string;
  modelName: string;
} | null>(() => {
  const { modelId, providerId } = props;
  if (modelId === "") return null;
  const own = groups.value.find((group) => group.provider === providerId);
  const candidates = own ? [own] : providerId === "" ? groups.value : [];
  for (const group of candidates) {
    const hit = group.models.find((model) => model.id === modelId);
    if (hit) {
      return {
        provider: group.provider,
        providerName: group.name,
        modelName: hit.name,
      };
    }
  }
  return null;
});

/** The select value: the resolved provider and model, empty when unresolved. */
const selectedValue = computed(() =>
  selected.value === null
    ? ""
    : modelValueOf(selected.value.provider, props.modelId),
);

/** The muted prefix: the provider of the resolved choice, else the stored id. */
const selectedProviderName = computed(
  () => selected.value?.providerName ?? props.providerId,
);

/** Emits the choice in decoded form, so the panel saves both halves at once. */
function handleSelect(value: string) {
  const choice = modelChoiceOf(value);
  if (choice === null) return;
  emit("selectModel", choice);
}
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
        <!-- Model selector (S2 of `models-by-provider.md`): grouped by provider,
             one item per enabled model. The value carries provider + model, and
             the trigger reads as "provider · model". -->
        <Select
          :model-value="selectedValue"
          :disabled="modelDisabled"
          @update:model-value="handleSelect(String($event))"
        >
          <SelectTrigger
            class="h-8 w-[150px] min-w-0 gap-1 border-0 bg-transparent text-xs shadow-none hover:bg-muted sm:w-[196px] dark:bg-transparent dark:hover:bg-muted"
            :aria-label="t('chat.composer.model')"
          >
            <span
              v-if="selectedProviderName"
              class="shrink-0 truncate text-muted-foreground"
            >
              {{ selectedProviderName }}
            </span>
            <!-- A model the list does not carry (a file edit, a disabled mark)
                 is still named instead of being hidden behind the placeholder. -->
            <SelectValue
              v-if="selected !== null || modelId === ''"
              :placeholder="t('chat.composer.selectModel')"
            />
            <span v-else class="min-w-0 truncate" :title="modelId">
              {{ modelId }}
            </span>
          </SelectTrigger>
          <SelectContent>
            <SelectGroup
              v-for="group in groups"
              :key="group.provider"
            >
              <SelectLabel>{{ group.name }}</SelectLabel>
              <SelectItem
                v-for="model in group.models"
                :key="modelValueOf(group.provider, model.id)"
                :value="modelValueOf(group.provider, model.id)"
              >
                {{ model.name }}
              </SelectItem>
            </SelectGroup>
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
