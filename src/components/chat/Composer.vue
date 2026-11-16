<script setup lang="ts">
// Composer card (spec S2): auto-growing prompt, inert "+" and scope chip, the
// provider → model selectors and the circular send/stop control.
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
  SelectItem,
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
 * One row of the provider select: a provider that has something to offer. The
 * label is pi's own provider name and falls back to the id when it is empty.
 */
interface ProviderOption {
  id: string;
  name: string;
  models: ProviderModels["models"];
}

const props = defineProps<{
  /** Prompt text (v-model). */
  modelValue: string;
  /**
   * Providers with their models, from `list_provider_models` (S1 of
   * `providers-pane-refinements.md`). The UI never hardcodes a model id: both
   * selects paint exactly what pi's file holds.
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
  /**
   * Provider/model selection is unavailable (saving, or no enabled model
   * reported): both selects move together, so both are disabled at once.
   */
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
 * S1 of `providers-pane-refinements.md`: the footer carries two selects — the
 * provider on the left, that provider's models on the right — instead of one list
 * grouped by provider. `enabled === false` is hidden here, not deleted from the
 * file (L3), so a provider with no usable model is not painted at all.
 */
const providerOptions = computed<ProviderOption[]>(() =>
  props.providerModels
    .map((entry) => ({
      id: entry.provider,
      name: entry.name.trim() === "" ? entry.provider : entry.name,
      models: entry.models.filter((model) => model.enabled),
    }))
    .filter((option) => option.models.length > 0),
);

/**
 * The provider on screen: the persisted one while it still has models, otherwise
 * the first provider that has any (S1). That fallback is view-only — settings are
 * written only when the human picks, never on render.
 */
const activeProvider = computed<ProviderOption | null>(() => {
  const option = providerOptions.value.find(
    (candidate) => candidate.id === props.providerId,
  );
  return option ?? providerOptions.value[0] ?? null;
});

/**
 * The model on screen: the persisted id while the provider on screen carries it,
 * `""` otherwise, so the select shows its placeholder instead of a value from
 * another provider.
 */
const activeModelId = computed(() => {
  const provider = activeProvider.value;
  if (provider === null || props.modelId === "") return "";
  const hit = provider.models.find((model) => model.id === props.modelId);
  return hit === undefined ? "" : props.modelId;
});

/**
 * A persisted model the provider on screen does not carry (a file edit, a
 * disabled mark) is still named instead of being hidden behind the placeholder,
 * but only while the persisted provider is the one on screen: under the first-
 * provider fallback the stored model belongs elsewhere and naming it would lie.
 */
const unresolvedModelId = computed(() => {
  const provider = activeProvider.value;
  if (provider === null || provider.id !== props.providerId) return "";
  if (props.modelId === "" || activeModelId.value !== "") return "";
  return props.modelId;
});

/**
 * S1: changing the provider keeps the model when that provider offers it (model
 * ids are provider-scoped, so both halves move together) and takes the first
 * enabled model otherwise. One choice per interaction, so the panel saves the
 * pair with one write; a selection that is already stored writes nothing.
 */
function handleSelectProvider(value: string) {
  const option = providerOptions.value.find(
    (candidate) => candidate.id === value,
  );
  if (option === undefined) return;
  const keep = option.models.some((model) => model.id === props.modelId)
    ? props.modelId
    : option.models[0].id;
  if (option.id === props.providerId && keep === props.modelId) return;
  emit("selectModel", { provider: option.id, model: keep });
}

/** S1: changing the model persists the pair with the provider on screen. */
function handleSelectModel(value: string) {
  const provider = activeProvider.value;
  if (provider === null || value === "") return;
  if (provider.id === props.providerId && value === props.modelId) return;
  emit("selectModel", { provider: provider.id, model: value });
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
        <!-- Provider → model (S1 of `providers-pane-refinements.md`): the
             provider on the left, the enabled models of that provider on the
             right. Both triggers are compact (h-8, no border) and labelled, so
             a keyboard user can open and walk either of them. -->
        <Select
          :model-value="activeProvider?.id ?? ''"
          :disabled="modelDisabled || providerOptions.length === 0"
          @update:model-value="handleSelectProvider(String($event))"
        >
          <SelectTrigger
            size="sm"
            class="w-[104px] min-w-0 shrink gap-1 border-0 bg-transparent text-xs shadow-none hover:bg-muted sm:w-[124px] dark:bg-transparent dark:hover:bg-muted"
            :aria-label="t('chat.composer.provider')"
          >
            <SelectValue :placeholder="t('chat.composer.selectProvider')" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem
              v-for="option in providerOptions"
              :key="option.id"
              :value="option.id"
            >
              {{ option.name }}
            </SelectItem>
          </SelectContent>
        </Select>

        <Select
          :model-value="activeModelId"
          :disabled="modelDisabled || activeProvider === null"
          @update:model-value="handleSelectModel(String($event))"
        >
          <SelectTrigger
            size="sm"
            class="w-[124px] min-w-0 shrink gap-1 border-0 bg-transparent text-xs shadow-none hover:bg-muted sm:w-[168px] dark:bg-transparent dark:hover:bg-muted"
            :aria-label="t('chat.composer.model')"
          >
            <!-- A model the provider does not carry is still named instead of
                 being hidden behind the placeholder. -->
            <SelectValue
              v-if="unresolvedModelId === ''"
              :placeholder="t('chat.composer.selectModel')"
            />
            <span v-else class="min-w-0 truncate" :title="unresolvedModelId">
              {{ unresolvedModelId }}
            </span>
          </SelectTrigger>
          <SelectContent>
            <SelectItem
              v-for="model in activeProvider?.models ?? []"
              :key="model.id"
              :value="model.id"
            >
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
