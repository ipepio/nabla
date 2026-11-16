<script setup lang="ts">
// Composer card (spec S2): auto-growing prompt, inert "+" and scope chip, the
// provider → model picker and the circular send/stop control.
//
// Presentation only: every action is emitted, so ChatPanel keeps the single
// `sendPrompt` / `stopSession` path and this card never touches the contracts
// seam. Colours come from semantic tokens only (S4) — no hex here.
import { computed } from "vue";
import {
  ArrowUpIcon,
  CheckIcon,
  ChevronDownIcon,
  PlusIcon,
  SquareIcon,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
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
 * One row of the picker: a provider and the models it *can* offer. The label is
 * pi's own provider name and falls back to the id when it is empty.
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
   * `providers-pane-refinements.md`). The UI never hardcodes a model id: the
   * picker paints exactly what pi's file holds.
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
   * reported): the single trigger is disabled, so no half choice is possible.
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
 * One dropdown in the footer: every provider is a row that opens that provider's
 * models in a lateral submenu, so a pick is still one interaction (and one
 * settings write) for the provider + model pair. `enabled === false` is hidden
 * here, not deleted from the file (L3), so a disabled model is never offered.
 * Providers are all painted — a provider with no enabled model keeps its row and
 * is disabled instead of silently vanishing.
 */
const providerRows = computed<ProviderOption[]>(() =>
  props.providerModels.map((entry) => ({
    id: entry.provider,
    name: entry.name.trim() === "" ? entry.provider : entry.name,
    models: entry.models.filter((model) => model.enabled),
  })),
);

/** Rows that can actually be picked from: the trigger's enabled/fallback set. */
const providerOptions = computed<ProviderOption[]>(() =>
  providerRows.value.filter((option) => option.models.length > 0),
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
 * The model on screen: the persisted one while the provider on screen carries it,
 * `null` otherwise, so the trigger shows its placeholder instead of a value from
 * another provider.
 */
const activeModel = computed(() => {
  const provider = activeProvider.value;
  if (provider === null || props.modelId === "") return null;
  return provider.models.find((model) => model.id === props.modelId) ?? null;
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
  if (props.modelId === "" || activeModel.value !== null) return "";
  return props.modelId;
});

/**
 * What the trigger's model half reads: the model's display name while the pair
 * resolves, the raw persisted id while it does not, `""` when there is nothing
 * to show and the placeholder takes over.
 */
const activeModelLabel = computed(
  () => activeModel.value?.name ?? unresolvedModelId.value,
);

/** Hover/long-press text for the trigger: the full pair, never truncated. */
const triggerLabel = computed(() => {
  const provider = activeProvider.value;
  if (provider === null) return t("chat.composer.selectProvider");
  const model = activeModelLabel.value;
  return model === ""
    ? `${provider.name} · ${t("chat.composer.selectModel")}`
    : `${provider.name} · ${model}`;
});

/** The check mark follows the *persisted* pair, not the fallback on screen. */
function isSelectedPair(provider: string, model: string): boolean {
  return provider === props.providerId && model === props.modelId;
}

/**
 * Picking a model fixes both halves at once (model ids are provider-scoped), so
 * the panel saves the pair with one write; a pick that is already stored writes
 * nothing.
 */
function chooseModel(provider: string, model: string) {
  if (provider === "" || model === "") return;
  if (provider === props.providerId && model === props.modelId) return;
  emit("selectModel", { provider, model });
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
        <!-- Provider → model: one trigger naming the current pair (provider as
             a muted prefix), and one menu where each provider row opens its own
             enabled models laterally. Reka owns the keyboard walk: no custom
             click handler on the sub trigger, so arrows/hover still open it. -->
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button
              variant="ghost"
              size="sm"
              class="h-8 max-w-[220px] min-w-0 gap-1 border-0 bg-transparent px-2 text-xs font-normal shadow-none hover:bg-muted aria-expanded:bg-muted sm:max-w-[280px] dark:bg-transparent dark:hover:bg-muted"
              :disabled="modelDisabled || providerOptions.length === 0"
              :aria-label="t('chat.composer.model')"
              :title="triggerLabel"
            >
              <template v-if="activeProvider !== null">
                <span class="shrink-0 text-muted-foreground">
                  {{ activeProvider.name }}
                </span>
                <!-- A model the provider does not carry is still named instead
                     of being hidden behind the placeholder. -->
                <span
                  v-if="activeModelLabel !== ''"
                  class="min-w-0 truncate"
                >
                  {{ activeModelLabel }}
                </span>
                <span v-else class="min-w-0 truncate text-muted-foreground">
                  {{ t("chat.composer.selectModel") }}
                </span>
              </template>
              <span v-else class="min-w-0 truncate text-muted-foreground">
                {{ t("chat.composer.selectProvider") }}
              </span>
              <ChevronDownIcon class="size-3 shrink-0 opacity-60" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            side="top"
            class="w-56"
            :aria-label="t('chat.composer.provider')"
          >
            <DropdownMenuSub
              v-for="row in providerRows"
              :key="row.id"
            >
              <!-- Disabled, not removed: a provider with no enabled model keeps
                   its row but cannot be opened. -->
              <DropdownMenuSubTrigger
                :disabled="row.models.length === 0"
                class="text-xs data-[disabled]:opacity-50"
              >
                <span class="min-w-0 truncate">{{ row.name }}</span>
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent class="w-56 max-h-[280px] overflow-y-auto">
                <DropdownMenuItem
                  v-for="model in row.models"
                  :key="model.id"
                  class="text-xs"
                  @select="chooseModel(row.id, model.id)"
                >
                  <CheckIcon
                    v-if="isSelectedPair(row.id, model.id)"
                    class="size-3.5 shrink-0"
                  />
                  <span v-else class="size-3.5 shrink-0" aria-hidden="true" />
                  <span class="min-w-0 truncate">{{ model.name }}</span>
                </DropdownMenuItem>
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          </DropdownMenuContent>
        </DropdownMenu>

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
