<script setup lang="ts">
/**
 * Per-model editor of the Providers pane (S3 of
 * `odd/tasks/models-by-provider.md`).
 *
 * One collapsible block per provider of pi's `models.json`, collapsed by default
 * and headed by the provider's name and its model count. Inside, one row per
 * model edits what pi reads for it: the name, the context window, the max output
 * tokens, the input and output cost, the input modalities (`text`, `image`) and
 * the `enabled` mark (L3: disabling keeps the entry in the file, it never
 * deletes it).
 *
 * The whole surface is built on the frozen contract: `list_provider_models` for
 * the read, `update_provider_model` for a row and `remove_provider_model` for the
 * removal behind a confirmation. Every write answers with what the file now
 * holds, so the block is rebuilt from the file after a save or a removal instead
 * of patching a local guess.
 *
 * Errors are per block and never touch the form: a rejected write leaves every
 * typed value where it is, so fixing one field costs a retry, not a retyped row.
 * Only the provider that was written is rebuilt, so edits in progress in another
 * block survive a save next to them.
 */
import { computed, onMounted, ref, watch } from "vue";
import { ChevronDownIcon } from "@lucide/vue";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  commandErrorMessage,
  listProviderModels,
  removeProviderModel,
  updateProviderModel,
} from "@/contracts/commands";
import type { ProviderModel, ProviderSummary } from "@/contracts/types";
import { useI18n } from "@/lib/i18n";

/**
 * The pane's provider list. It is not what this editor renders (the blocks come
 * from `list_provider_models`, which also carries providers with no models); it
 * is the signal to re-read after a provider is added or removed, so a removed
 * provider cannot leave a block behind that no longer exists in the file.
 */
const props = defineProps<{ providers: ProviderSummary[] }>();

/**
 * S3: a write landed, so the pane above can re-read its own summaries — the model
 * count of a row is the one thing this editor cannot refresh from inside.
 */
const emit = defineEmits<{ changed: [] }>();

/** S3 of `i18n-and-settings-polish.md`: every visible string, both locales. */
const { t } = useI18n();

/** One model row as the form holds it: numbers are text so empty stays empty. */
interface ModelDraft {
  name: string;
  contextWindow: string;
  maxTokens: string;
  inputCost: string;
  outputCost: string;
  modalities: { text: boolean; image: boolean };
  enabled: boolean;
}

interface ModelRow {
  id: string;
  draft: ModelDraft;
}

interface ProviderBlock {
  provider: string;
  name: string;
  models: ModelRow[];
}

/** The numeric fields, in the order the row paints them. */
const NUMERIC_FIELDS = [
  "contextWindow",
  "maxTokens",
  "inputCost",
  "outputCost",
] as const;
type NumericField = (typeof NUMERIC_FIELDS)[number];

const blocks = ref<ProviderBlock[]>([]);
const loading = ref(false);
/** The provider whose block is open; every block starts collapsed. */
const expanded = ref<Record<string, boolean>>({});
/** Inline error per provider: the block that failed, not the whole pane. */
const errors = ref<Record<string, string>>({});
/** The row a write is in flight for (`<provider>::<modelId>`), so it cannot fire twice. */
const busy = ref<string | null>(null);
/** The model the removal confirmation is about, `null` when it is closed. */
const pendingRemoval = ref<{ provider: string; modelId: string } | null>(null);
const removalOpen = ref(false);
const removing = ref(false);

function rowKey(provider: string, modelId: string): string {
  return `${provider}::${modelId}`;
}

/** A stable, HTML-safe id: model ids may carry `/` and `.`. */
function fieldId(provider: string, modelId: string, field: string): string {
  const slug = `${provider}-${modelId}-${field}`.replace(/[^a-zA-Z0-9_-]+/g, "-");
  return `model-setting-${slug}`;
}

/** The form values of one model, as the file reports it. */
function draftOf(model: ProviderModel): ModelDraft {
  const numeric = (value: number | null): string =>
    value === null ? "" : String(value);
  return {
    name: model.name,
    contextWindow: numeric(model.contextWindow),
    maxTokens: numeric(model.maxTokens),
    inputCost: numeric(model.inputCost),
    outputCost: numeric(model.outputCost),
    modalities: {
      text: model.modalities.includes("text"),
      image: model.modalities.includes("image"),
    },
    enabled: model.enabled,
  };
}

/** One block from one provider of the read, drafts included. */
function blockOf(entry: {
  provider: string;
  name: string;
  models: ProviderModel[];
}): ProviderBlock {
  return {
    provider: entry.provider,
    name: entry.name.trim() === "" ? entry.provider : entry.name,
    models: entry.models.map((model) => ({
      id: model.id,
      draft: draftOf(model),
    })),
  };
}

function isOpen(provider: string): boolean {
  return expanded.value[provider] === true;
}

function toggle(provider: string) {
  expanded.value = { ...expanded.value, [provider]: !isOpen(provider) };
}

function setError(provider: string, message: string) {
  errors.value = { ...errors.value, [provider]: message };
}

function clearError(provider: string) {
  if (errors.value[provider] === undefined) return;
  const next = { ...errors.value };
  delete next[provider];
  errors.value = next;
}

/**
 * S3: the read the whole editor is built on. A failure is shown once, above the
 * blocks, because there is no row yet to attach it to.
 */
async function load() {
  loading.value = true;
  try {
    const list = await listProviderModels();
    blocks.value = list.map(blockOf);
    errors.value = {};
  } catch (cause) {
    blocks.value = [];
    errors.value = { "": commandErrorMessage(cause) };
  } finally {
    loading.value = false;
  }
}

/**
 * Re-reads the file and rebuilds one provider only. The written row is taken from
 * the answer, so it shows what the file now holds; every row the human is still
 * editing keeps what was typed, and a row the file no longer has disappears.
 *
 * `syncedModelId` is the row the write just answered for, `null` for a removal
 * (nothing was written, so no row is replaced).
 */
async function refreshProvider(
  provider: string,
  syncedModelId: string | null,
) {
  const list = await listProviderModels();
  const fresh = list.find((entry) => entry.provider === provider);
  if (fresh === undefined) {
    blocks.value = blocks.value.filter((block) => block.provider !== provider);
    return;
  }
  const applied = blockOf(fresh);
  const current = blocks.value.find((block) => block.provider === provider);
  if (current !== undefined) {
    applied.models = applied.models.map((row) => {
      if (row.id === syncedModelId) return row;
      return current.models.find((existing) => existing.id === row.id) ?? row;
    });
  }
  blocks.value = blocks.value.map((block) =>
    block.provider === provider ? applied : block,
  );
}

/**
 * "Empty means unset" for a numeric field: the draft sends `null` and Rust keeps
 * the value the file already holds. `undefined` is "not a number", which fails
 * inline instead of being guessed into the file.
 */
function numberValue(raw: string): number | null | undefined {
  const text = raw.trim();
  if (text === "") return null;
  const value = Number(text);
  if (!Number.isFinite(value) || value < 0) return undefined;
  return value;
}

/** The modalities the file stores as its `input` array. */
function modalitiesOf(draft: ModelDraft): string[] {
  const picked: string[] = [];
  if (draft.modalities.text) picked.push("text");
  if (draft.modalities.image) picked.push("image");
  return picked;
}

/**
 * S3: writes one model inside its provider. A value that is not a number stops
 * the call and fails inline, so a typo never reaches the file as a guess.
 */
async function handleSave(block: ProviderBlock, row: ModelRow) {
  if (busy.value !== null) return;
  const numbers = {} as Record<NumericField, number | null>;
  for (const field of NUMERIC_FIELDS) {
    const value = numberValue(row.draft[field]);
    if (value === undefined) {
      setError(block.provider, t("settings.providers.modelsEditor.invalidNumber"));
      return;
    }
    numbers[field] = value;
  }
  busy.value = rowKey(block.provider, row.id);
  clearError(block.provider);
  try {
    await updateProviderModel({
      provider: block.provider,
      id: row.id,
      name: row.draft.name.trim(),
      contextWindow: numbers.contextWindow,
      maxTokens: numbers.maxTokens,
      inputCost: numbers.inputCost,
      outputCost: numbers.outputCost,
      modalities: modalitiesOf(row.draft),
      enabled: row.draft.enabled,
    });
    await refreshProvider(block.provider, row.id);
    emit("changed");
  } catch (cause) {
    // A rejected write keeps every typed value: one field needs fixing.
    setError(block.provider, commandErrorMessage(cause));
  } finally {
    busy.value = null;
  }
}

/** Opens the confirmation of one model (S3). */
function askRemove(provider: string, modelId: string) {
  pendingRemoval.value = { provider, modelId };
  removalOpen.value = true;
}

/**
 * S3: removes the model after confirmation and re-reads the file. A model the
 * file no longer has is a no-op on the Rust side (`false`), so a stale row
 * disappears instead of turning into an error.
 */
async function confirmRemove() {
  const target = pendingRemoval.value;
  removalOpen.value = false;
  pendingRemoval.value = null;
  if (target === null || removing.value) return;
  removing.value = true;
  clearError(target.provider);
  try {
    await removeProviderModel(target.provider, target.modelId);
    await refreshProvider(target.provider, null);
    emit("changed");
  } catch (cause) {
    setError(target.provider, commandErrorMessage(cause));
  } finally {
    removing.value = false;
  }
}

/** The confirmation names the model and the provider it belongs to. */
const removalText = computed(() => {
  const target = pendingRemoval.value;
  if (target === null) return "";
  return t("settings.providers.modelsEditor.remove.description", {
    modelId: target.modelId,
    provider: target.provider,
  });
});

onMounted(() => {
  void load();
});

/**
 * A provider added or removed through the pane changes what this editor should
 * show, so the read is repeated when the id set moves. Nothing is compared field
 * by field: the ids are the list this component renders blocks for.
 */
watch(
  () => props.providers.map((entry) => entry.id).join("\n"),
  () => {
    void load();
  },
);
</script>

<template>
  <section class="space-y-2">
    <div class="min-w-0">
      <h3 class="text-sm font-medium">
        {{ t("settings.providers.modelsEditor.title") }}
      </h3>
      <p class="mt-0.5 text-xs text-muted-foreground">
        {{ t("settings.providers.modelsEditor.description") }}
      </p>
    </div>

    <p v-if="loading" class="text-xs text-muted-foreground">
      {{ t("common.loading") }}
    </p>

    <Alert v-else-if="errors['']" variant="destructive">
      <AlertDescription>{{ errors[""] }}</AlertDescription>
    </Alert>

    <!-- One block per provider, collapsed by default: the pane stays a list
         until one provider is opened. -->
    <div v-else-if="blocks.length" class="space-y-1">
      <div
        v-for="block in blocks"
        :key="block.provider"
        class="rounded-md border border-border"
      >
        <button
          type="button"
          :aria-expanded="isOpen(block.provider)"
          class="flex w-full items-center justify-between gap-2 rounded-sm px-2.5 py-2 text-left outline-hidden transition-colors hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
          @click="toggle(block.provider)"
        >
          <span class="flex min-w-0 items-baseline gap-2">
            <span class="min-w-0 truncate text-sm font-medium">
              {{ block.name }}
            </span>
            <span class="min-w-0 truncate font-mono text-xs text-muted-foreground">
              {{ block.provider }}
            </span>
          </span>
          <span class="flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
            {{ t("settings.providers.models", { count: block.models.length }) }}
            <ChevronDownIcon
              class="size-4 shrink-0 transition-transform"
              :class="isOpen(block.provider) ? 'rotate-180' : ''"
              aria-hidden="true"
            />
          </span>
        </button>

        <div
          v-if="isOpen(block.provider)"
          class="space-y-2 border-t border-border p-2.5"
        >
          <!-- S3: a provider the file lists without models still shows its block,
               so "add one" is not hidden behind a missing section. -->
          <p v-if="block.models.length === 0" class="text-xs text-muted-foreground">
            {{ t("settings.providers.modelsEditor.empty") }}
          </p>

          <div
            v-for="row in block.models"
            :key="row.id"
            class="space-y-2 rounded-md border border-border p-2"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="min-w-0 truncate font-mono text-xs text-muted-foreground">
                {{ row.id }}
              </span>
              <!-- L3: the enabled mark is written to the file; nothing is deleted. -->
              <div class="flex shrink-0 items-center gap-1.5">
                <span class="text-xs text-muted-foreground">
                  {{ t("settings.providers.modelsEditor.enabled") }}
                </span>
                <button
                  type="button"
                  role="switch"
                  :aria-checked="row.draft.enabled"
                  :aria-label="
                    t('settings.providers.modelsEditor.enabledFor', {
                      model: row.id,
                    })
                  "
                  :class="[
                    'relative inline-flex h-5 w-9 shrink-0 items-center rounded-full outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
                    row.draft.enabled ? 'bg-primary' : 'bg-input',
                  ]"
                  @click="row.draft.enabled = !row.draft.enabled"
                >
                  <span
                    :class="[
                      'size-4 rounded-full bg-background shadow-sm transition-transform',
                      row.draft.enabled ? 'translate-x-4.5' : 'translate-x-0.5',
                    ]"
                    aria-hidden="true"
                  />
                </button>
              </div>
            </div>

            <div class="space-y-1">
              <label
                :for="fieldId(block.provider, row.id, 'name')"
                class="text-xs font-medium"
              >
                {{ t("settings.providers.modelsEditor.name") }}
              </label>
              <Input
                :id="fieldId(block.provider, row.id, 'name')"
                v-model="row.draft.name"
                autocomplete="off"
                spellcheck="false"
                :aria-label="
                  `${t('settings.providers.modelsEditor.name')} · ${row.id}`
                "
              />
            </div>

            <!-- Numeric fields: empty is "not covered by this form", which the
                 contract sends as `null` and Rust merges as "keep the file's". -->
            <div class="grid gap-2 sm:grid-cols-2">
              <div
                v-for="field in NUMERIC_FIELDS"
                :key="field"
                class="space-y-1"
              >
                <label
                  :for="fieldId(block.provider, row.id, field)"
                  class="text-xs font-medium"
                >
                  {{ t(`settings.providers.modelsEditor.${field}`) }}
                </label>
                <Input
                  :id="fieldId(block.provider, row.id, field)"
                  v-model="row.draft[field]"
                  type="number"
                  min="0"
                  :step="
                    field === 'inputCost' || field === 'outputCost'
                      ? '0.01'
                      : '1'
                  "
                  inputmode="decimal"
                  :placeholder="t('settings.providers.modelsEditor.notSet')"
                  :aria-label="
                    `${t(`settings.providers.modelsEditor.${field}`)} · ${row.id}`
                  "
                />
              </div>
            </div>

            <div class="flex flex-wrap items-center gap-x-4 gap-y-1">
              <span class="text-xs font-medium">
                {{ t("settings.providers.modelsEditor.modalities") }}
              </span>
              <label class="flex items-center gap-1.5 text-xs">
                <input
                  v-model="row.draft.modalities.text"
                  type="checkbox"
                  class="size-3.5 shrink-0 accent-primary"
                  :aria-label="
                    t('settings.providers.modelsEditor.modality.textFor', {
                      model: row.id,
                    })
                  "
                />
                {{ t("settings.providers.modelsEditor.modality.text") }}
              </label>
              <label class="flex items-center gap-1.5 text-xs">
                <input
                  v-model="row.draft.modalities.image"
                  type="checkbox"
                  class="size-3.5 shrink-0 accent-primary"
                  :aria-label="
                    t('settings.providers.modelsEditor.modality.imageFor', {
                      model: row.id,
                    })
                  "
                />
                {{ t("settings.providers.modelsEditor.modality.image") }}
              </label>
            </div>

            <div class="flex items-center justify-end gap-2">
              <Button
                type="button"
                size="sm"
                :disabled="busy !== null || removing"
                @click="handleSave(block, row)"
              >
                {{
                  busy === rowKey(block.provider, row.id)
                    ? t("settings.providers.modelsEditor.saving")
                    : t("settings.providers.modelsEditor.save")
                }}
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                :disabled="busy !== null || removing"
                @click="askRemove(block.provider, row.id)"
              >
                {{ t("settings.providers.modelsEditor.remove") }}
              </Button>
            </div>
          </div>

          <!-- Inline, per block: the other providers stay usable. -->
          <Alert v-if="errors[block.provider]" variant="destructive">
            <AlertDescription>{{ errors[block.provider] }}</AlertDescription>
          </Alert>
        </div>
      </div>
    </div>

    <!-- One confirmation for the whole editor: the trigger only says which model
         it is about. -->
    <Dialog v-model:open="removalOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            {{ t("settings.providers.modelsEditor.remove.title") }}
          </DialogTitle>
          <DialogDescription>{{ removalText }}</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <DialogClose as-child>
            <Button variant="outline">
              {{ t("common.cancel") }}
            </Button>
          </DialogClose>
          <Button variant="destructive" :disabled="removing" @click="confirmRemove">
            {{ t("settings.providers.remove") }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </section>
</template>
