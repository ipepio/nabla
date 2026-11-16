<script setup lang="ts">
/**
 * Models of one provider (S3 of `odd/tasks/models-by-provider.md`, S4 of
 * `odd/tasks/providers-pane-refinements.md`).
 *
 * S4: this editor is no longer a standalone block of the pane. It is rendered
 * inside the provider's edit dialog (`SettingsDialog.vue`), for exactly the
 * provider being edited: the models of a provider sit next to that provider
 * instead of in a second list that has to repeat every provider.
 *
 * One row per model edits what pi reads for it: the name, the context window, the
 * max output tokens, the input and output cost, the input modalities (`text`,
 * `image`) and the `enabled` mark (L3: disabling keeps the entry in the file, it
 * never deletes it). A provider with no models in the file says so and offers
 * nothing else — new models come from the custom-provider form's discovery.
 *
 * The whole surface is built on the frozen contract: `list_provider_models` for
 * the read, `update_provider_model` for a row and `remove_provider_model` for the
 * removal behind a confirmation. Every write answers with what the file now
 * holds, so the row is rebuilt from the file after a save or a removal instead of
 * patching a local guess, and the parent refreshes its model count through
 * `changed`.
 *
 * S5 of `providers-pane-refinements.md` (user feedback): the row's name is an
 * edited field like every other one — it travels in the same
 * `update_provider_model` draft — and the dialog can also ADD models a provider
 * offers but the file does not list yet. "Discover models" asks the endpoint
 * through `discover_provider_models` (Rust resolves the base URL and api key from
 * pi's file, so no secret crosses the seam), shows what is NOT configured yet as
 * a checklist, and adds the ticked ones one `update_provider_model` per model.
 * Existing entries are filtered out before they are even offered and re-checked
 * against a fresh read before the write, so this action never overwrites a
 * configured model.
 *
 * Errors are one line above the rows: a rejected write leaves every typed value
 * where it is, so fixing one field costs a retry, not a retyped row.
 *
 * Layout: rows carry no rule of their own and no border; the dialog's own
 * spacing separates them.
 */
import { computed, ref, watch } from "vue";
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
  discoverProviderModels,
  listProviderModels,
  removeProviderModel,
  updateProviderModel,
} from "@/contracts/commands";
import type {
  ModelOption,
  ProviderModel,
  ProviderSummary,
} from "@/contracts/types";
import { useI18n } from "@/lib/i18n";

/** S4: the provider being edited — the only one this editor reads and writes. */
const props = defineProps<{ provider: ProviderSummary }>();

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

/** The numeric fields, in the order the row paints them. */
const NUMERIC_FIELDS = [
  "contextWindow",
  "maxTokens",
  "inputCost",
  "outputCost",
] as const;
type NumericField = (typeof NUMERIC_FIELDS)[number];

const models = ref<ModelRow[]>([]);
const loading = ref(false);
/** The one error line of this editor: the read, or the last write, failed. */
const error = ref<string | null>(null);
/** The model a write is in flight for, so the row cannot fire twice. */
const busy = ref<string | null>(null);
/** The model the removal confirmation is about, `null` when it is closed. */
const pendingRemoval = ref<string | null>(null);
const removalOpen = ref(false);
const removing = ref(false);

/** S5: what the endpoint answered, narrowed to the models the file does not list. */
const discovered = ref<ModelOption[]>([]);
/** One tick per discovered model, keyed by id; only the unconfigured ones exist here. */
const selected = ref<Record<string, boolean>>({});
const discovering = ref(false);
const adding = ref(false);
/** Discovery ran for this provider, so "nothing new" is an answer and not a silence. */
const discoveryRan = ref(false);

/** S5: the ticked models, in the order the endpoint answered them. */
const selectedDiscovered = computed(() =>
  discovered.value.filter((option) => selected.value[option.id] === true),
);

/** A stable, HTML-safe id: model ids may carry `/` and `.`. */
function fieldId(modelId: string, field: string): string {
  const slug = `${props.provider.id}-${modelId}-${field}`.replace(
    /[^a-zA-Z0-9_-]+/g,
    "-",
  );
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

/**
 * S3: the read the editor is built on.
 *
 * `syncedModelId` is the row a write just answered for (`null` for a removal):
 * that row is taken from the answer, so it shows what the file now holds, while
 * every row still being edited keeps what was typed and a row the file no longer
 * has disappears. `silent` is the refresh that follows a write: the rows stay on
 * screen instead of flashing back to the loading line.
 */
async function load(syncedModelId: string | null = null, silent = false) {
  if (!silent) loading.value = true;
  try {
    const list = await listProviderModels();
    const entry = list.find((item) => item.provider === props.provider.id);
    const previous = models.value;
    models.value = (entry?.models ?? []).map((model) => {
      const fresh: ModelRow = { id: model.id, draft: draftOf(model) };
      if (model.id === syncedModelId) return fresh;
      return previous.find((existing) => existing.id === model.id) ?? fresh;
    });
    error.value = null;
  } catch (cause) {
    // A failed first read has no rows to keep; a failed refresh keeps the ones
    // on screen and only reports the reason.
    if (!silent) models.value = [];
    error.value = commandErrorMessage(cause);
  } finally {
    if (!silent) loading.value = false;
  }
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
async function handleSave(row: ModelRow) {
  if (busy.value !== null || removing.value) return;
  const numbers = {} as Record<NumericField, number | null>;
  for (const field of NUMERIC_FIELDS) {
    const value = numberValue(row.draft[field]);
    if (value === undefined) {
      error.value = t("settings.providers.modelsEditor.invalidNumber");
      return;
    }
    numbers[field] = value;
  }
  busy.value = row.id;
  error.value = null;
  try {
    await updateProviderModel({
      provider: props.provider.id,
      id: row.id,
      name: row.draft.name.trim(),
      contextWindow: numbers.contextWindow,
      maxTokens: numbers.maxTokens,
      inputCost: numbers.inputCost,
      outputCost: numbers.outputCost,
      modalities: modalitiesOf(row.draft),
      enabled: row.draft.enabled,
    });
    await load(row.id, true);
    emit("changed");
  } catch (cause) {
    // A rejected write keeps every typed value: one field needs fixing.
    error.value = commandErrorMessage(cause);
  } finally {
    busy.value = null;
  }
}

/**
 * S5: asks the provider's endpoint for its model list. Rust resolves the base URL
 * and the api key from pi's own file, so the webview never handles the secret.
 *
 * Only models the file does not list yet are offered: adding is an additive
 * action, so an entry that already exists is never a candidate. A failure keeps
 * the rows and their drafts exactly as they are and reports its reason inline;
 * the checklist is dropped because it would describe an answer that no longer
 * stands.
 */
async function handleDiscover() {
  if (
    discovering.value ||
    adding.value ||
    busy.value !== null ||
    removing.value
  ) {
    return;
  }
  discovering.value = true;
  error.value = null;
  try {
    const options = await discoverProviderModels(props.provider.id);
    const configured = new Set(models.value.map((row) => row.id));
    const fresh = options.filter((option) => !configured.has(option.id));
    discovered.value = fresh;
    selected.value = Object.fromEntries(
      fresh.map((model) => [model.id, true]),
    );
    discoveryRan.value = true;
  } catch (cause) {
    discovered.value = [];
    selected.value = {};
    discoveryRan.value = false;
    error.value = commandErrorMessage(cause);
  } finally {
    discovering.value = false;
  }
}

/**
 * S5: adds the ticked models, one `update_provider_model` per model, with the
 * name the discovery answer carries. Empty numbers and modalities mean "keep what
 * the file has" in that draft; for a new entry Rust writes the id, the name and
 * the enabled mark, which is exactly what a discovered model has.
 *
 * Nothing configured is ever overwritten: the ticked ids are checked against a
 * FRESH read first, so a model added in another window between the discovery and
 * this click is skipped instead of replaced. A mid-way failure keeps the
 * checklist, so the retry costs no second discovery and skips what already landed.
 */
async function handleAddDiscovered() {
  const picks = selectedDiscovered.value;
  if (
    picks.length === 0 ||
    adding.value ||
    discovering.value ||
    busy.value !== null ||
    removing.value
  ) {
    return;
  }
  adding.value = true;
  error.value = null;
  try {
    const list = await listProviderModels();
    const entry = list.find((item) => item.provider === props.provider.id);
    const configured = new Set((entry?.models ?? []).map((model) => model.id));
    for (const model of picks) {
      if (configured.has(model.id)) continue;
      await updateProviderModel({
        provider: props.provider.id,
        id: model.id,
        name: model.name,
        contextWindow: null,
        maxTokens: null,
        inputCost: null,
        outputCost: null,
        modalities: [],
        enabled: true,
      });
      configured.add(model.id);
    }
    await load(null, true);
    emit("changed");
    // The checklist was about what the file did not list: it is spent now.
    discovered.value = [];
    selected.value = {};
    discoveryRan.value = false;
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    adding.value = false;
  }
}

/** Opens the confirmation of one model (S3). */
function askRemove(modelId: string) {
  pendingRemoval.value = modelId;
  removalOpen.value = true;
  error.value = null;
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
  error.value = null;
  try {
    await removeProviderModel(props.provider.id, target);
    await load(null, true);
    emit("changed");
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    removing.value = false;
  }
}

/** The confirmation names the model and the provider it belongs to. */
const removalText = computed(() => {
  const target = pendingRemoval.value;
  if (target === null) return "";
  return t("settings.providers.modelsEditor.remove.description", {
    modelId: target,
    provider: props.provider.id,
  });
});

// S4: one dialog per provider, so the read follows the provider it was opened
// for. A different id is a different set of models, never a merge of both — and
// the same rule applies to a discovery made for the previous provider.
watch(
  () => props.provider.id,
  () => {
    discovered.value = [];
    selected.value = {};
    discoveryRan.value = false;
    void load();
  },
  { immediate: true },
);
</script>

<template>
  <section class="space-y-3">
    <div class="min-w-0">
      <h4 class="text-sm font-medium">
        {{ t("settings.providers.modelsEditor.heading") }}
      </h4>
      <p class="mt-0.5 text-xs text-muted-foreground">
        {{ t("settings.providers.modelsEditor.description") }}
      </p>
    </div>

    <p v-if="loading" class="text-xs text-muted-foreground">
      {{ t("common.loading") }}
    </p>

    <!-- S3: a provider the file lists without models still shows the block, so
         the human sees "it has none" instead of a blank area. -->
    <p
      v-else-if="models.length === 0 && !error"
      class="text-xs text-muted-foreground"
    >
      {{ t("settings.providers.modelsEditor.empty") }}
    </p>

    <!-- One model per group, separated by spacing only. -->
    <div v-else class="space-y-4">
      <div v-for="row in models" :key="row.id" class="space-y-2">
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
          <label :for="fieldId(row.id, 'name')" class="text-xs font-medium">
            {{ t("settings.providers.modelsEditor.name") }}
          </label>
          <Input
            :id="fieldId(row.id, 'name')"
            v-model="row.draft.name"
            autocomplete="off"
            spellcheck="false"
            :aria-label="`${t('settings.providers.modelsEditor.name')} · ${row.id}`"
          />
        </div>

        <!-- Numeric fields: empty is "not covered by this form", which the
             contract sends as `null` and Rust merges as "keep the file's". -->
        <div class="grid gap-2 sm:grid-cols-2">
          <div v-for="field in NUMERIC_FIELDS" :key="field" class="space-y-1">
            <label :for="fieldId(row.id, field)" class="text-xs font-medium">
              {{ t(`settings.providers.modelsEditor.${field}`) }}
            </label>
            <Input
              :id="fieldId(row.id, field)"
              v-model="row.draft[field]"
              type="number"
              min="0"
              :step="field === 'inputCost' || field === 'outputCost' ? '0.01' : '1'"
              inputmode="decimal"
              :placeholder="t('settings.providers.modelsEditor.notSet')"
              :aria-label="`${t(`settings.providers.modelsEditor.${field}`)} · ${row.id}`"
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
            @click="handleSave(row)"
          >
            {{
              busy === row.id
                ? t("settings.providers.modelsEditor.saving")
                : t("settings.providers.modelsEditor.save")
            }}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            :disabled="busy !== null || removing"
            @click="askRemove(row.id)"
          >
            {{ t("settings.providers.modelsEditor.remove") }}
          </Button>
        </div>
      </div>
    </div>

    <!-- S5: adding models the file does not list yet. The endpoint is asked
         explicitly (never on open), only the unconfigured models are offered, and
         the add itself writes one entry per ticked model. Nothing here replaces a
         configured model. -->
    <div class="space-y-2">
      <Button
        type="button"
        variant="outline"
        size="sm"
        :disabled="discovering || adding || busy !== null || removing"
        @click="handleDiscover"
      >
        {{
          discovering
            ? t("settings.providers.modelsEditor.discovery.discovering")
            : t("settings.providers.modelsEditor.discovery.discover")
        }}
      </Button>
      <p class="text-xs text-muted-foreground">
        {{ t("settings.providers.modelsEditor.discovery.description") }}
      </p>

      <!-- The count is what the checklist is about: how many of the endpoint's
           new models are ticked. -->
      <p v-if="discovered.length" class="text-xs text-muted-foreground">
        {{
          t("settings.providers.modelsEditor.discovery.selected", {
            count: selectedDiscovered.length,
            total: discovered.length,
          })
        }}
      </p>

      <div
        v-if="discovered.length"
        class="max-h-44 space-y-0.5 overflow-y-auto rounded-md border border-border p-1"
      >
        <label
          v-for="model in discovered"
          :key="model.id"
          class="flex items-center gap-2 rounded-sm px-1.5 py-1 text-sm hover:bg-muted"
        >
          <input
            v-model="selected[model.id]"
            type="checkbox"
            class="size-4 shrink-0 accent-primary"
            :aria-label="
              t('settings.providers.modelsEditor.discovery.include', {
                model: model.id,
              })
            "
          />
          <span class="min-w-0 truncate">{{ model.name }}</span>
          <span
            v-if="model.name !== model.id"
            class="ml-auto shrink-0 font-mono text-xs text-muted-foreground"
          >
            {{ model.id }}
          </span>
        </label>
      </div>

      <p v-else-if="discoveryRan" class="text-xs text-muted-foreground">
        {{ t("settings.providers.modelsEditor.discovery.allConfigured") }}
      </p>

      <div v-if="discovered.length" class="flex justify-end">
        <Button
          type="button"
          size="sm"
          :disabled="selectedDiscovered.length === 0 || adding"
          @click="handleAddDiscovered"
        >
          {{
            adding
              ? t("settings.providers.modelsEditor.discovery.adding")
              : t("settings.providers.modelsEditor.discovery.add", {
                  count: selectedDiscovered.length,
                })
          }}
        </Button>
      </div>
    </div>

    <!-- Inline: the failed read or write of this provider, next to its rows. -->
    <Alert v-if="error" variant="destructive">
      <AlertDescription>{{ error }}</AlertDescription>
    </Alert>

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
