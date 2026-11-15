<script setup lang="ts">
/**
 * Custom provider form (S2 of `odd/tasks/providers-pane.md`).
 *
 * S5: the parent renders this component only when `VITE_ENABLE_CUSTOM_PROVIDERS`
 * is on, so a build without the flag has no control whose backend is missing.
 *
 * S2: "descubrir modelos" asks Rust for `GET {baseUrl}/models` — the api key never
 * reaches the webview and CORS/CSP stay out of the way — and the answer becomes
 * checkboxes, all ticked. Both discovery and save are form actions: a failure
 * surfaces inline and leaves every typed field as it was, so a wrong key costs a
 * retry instead of a retyped form.
 *
 * The save call is `save_provider`, which writes into pi's own `providers.<id>`
 * (backup, merge, atomic write, S3/S4) and answers with the summary the file now
 * holds; the parent refreshes its list from that answer instead of guessing.
 */
import { computed, ref } from "vue";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useI18n } from "@/lib/i18n";
import {
  commandErrorMessage,
  discoverModels,
  saveProvider,
} from "@/contracts/commands";
import type { ModelOption, ProviderSummary } from "@/contracts/types";

/** The summary Rust wrote, so the pane refreshes from what the file holds. */
const emit = defineEmits<{
  saved: [summary: ProviderSummary];
  cancel: [];
}>();

/** S2 of `i18n-and-settings-polish.md`: every visible string, both locales. */
const { t } = useI18n();

/**
 * Provider presets: the endpoints that really speak the OpenAI wire format, so
 * starting a provider is a click instead of "type a base URL by hand" (the user
 * could not see how to sign in with OpenAI at all).
 *
 * Anthropic is deliberately absent: its native Messages API is not
 * OpenAI-compatible, so offering it here as one click would be a lie.
 *
 * Brand names stay literal — they are product names, not translated copy.
 */
const PROVIDER_PRESETS = [
  {
    value: "openai",
    label: "OpenAI",
    providerId: "openai",
    providerName: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
  },
  {
    value: "deepseek",
    label: "DeepSeek",
    providerId: "deepseek",
    providerName: "DeepSeek",
    baseUrl: "https://api.deepseek.com/v1",
  },
  {
    value: "gemini",
    label: "Google Gemini",
    providerId: "gemini",
    providerName: "Google Gemini",
    baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai",
  },
  {
    value: "nan",
    label: "NaN",
    providerId: "nan",
    providerName: "NaN",
    baseUrl: "https://api.nan.builders/v1",
  },
] as const;

/** The last preset picked. "custom" is the empty preset: no values to fill. */
const CUSTOM_PRESET = "custom";
const preset = ref<string>(CUSTOM_PRESET);

/**
 * The typed values. They are the form's own state and are never reset by a
 * failed discovery or a rejected save (S2).
 */
const providerId = ref("");
const providerName = ref("");
const baseUrl = ref("");
const apiKey = ref("");

/** What the last successful discovery returned, and the boxes ticked over it. */
const found = ref<ModelOption[]>([]);
const selected = ref<Record<string, boolean>>({});
const discovered = ref(false);

const discovering = ref(false);
const saving = ref(false);
const error = ref<string | null>(null);

const selectedModels = computed(() =>
  found.value.filter((model) => selected.value[model.id] === true),
);

/** The three fields the frozen contract rejects when empty. */
const requiredFilled = computed(
  () =>
    providerId.value.trim() !== "" &&
    baseUrl.value.trim() !== "" &&
    apiKey.value.trim() !== "",
);

/**
 * Copies one preset into the three fields it owns. It is a one-time fill, not a
 * binding: no watcher re-applies a preset when a field is edited afterwards, so
 * every value stays editable. "Custom" only clears the selection and never wipes
 * what the user typed.
 *
 * Picking a preset that changes the base URL also drops the models of the
 * previous endpoint: saving another endpoint's model ids would be silently wrong.
 */
function handlePresetChange(value: string) {
  const next = PROVIDER_PRESETS.find((entry) => entry.value === value);
  preset.value = next ? next.value : CUSTOM_PRESET;
  if (!next) return;
  if (baseUrl.value.trim() !== next.baseUrl) {
    found.value = [];
    selected.value = {};
    discovered.value = false;
  }
  providerId.value = next.providerId;
  providerName.value = next.providerName;
  baseUrl.value = next.baseUrl;
}

/** Discovery needs the two values it sends, and only one run at a time. */
const canDiscover = computed(
  () =>
    baseUrl.value.trim() !== "" && apiKey.value.trim() !== "" && !discovering.value,
);

const canSave = computed(
  () => requiredFilled.value && !saving.value && !discovering.value,
);

/**
 * S2: calls the endpoint through Rust and renders the answer as checked boxes.
 * A failure drops the stale model list but keeps every typed field.
 */
async function handleDiscover() {
  if (!canDiscover.value) return;
  discovering.value = true;
  error.value = null;
  try {
    const models = await discoverModels(
      baseUrl.value.trim(),
      apiKey.value.trim(),
    );
    found.value = models;
    selected.value = Object.fromEntries(
      models.map((model) => [model.id, true]),
    );
    discovered.value = true;
    if (models.length === 0) {
      error.value = t("settings.providers.custom.modelsEmpty");
    }
  } catch (cause) {
    found.value = [];
    selected.value = {};
    discovered.value = false;
    error.value = commandErrorMessage(cause);
  } finally {
    discovering.value = false;
  }
}

/**
 * S2/S3: saves the provider with the models that are ticked. `api` stays empty so
 * Rust keeps the value the file already has (and defaults a new provider to
 * `openai-completions`), and an empty name falls back to the id there too.
 */
async function handleSave() {
  if (!canSave.value) return;
  saving.value = true;
  error.value = null;
  try {
    const summary = await saveProvider({
      id: providerId.value.trim(),
      name: providerName.value.trim(),
      baseUrl: baseUrl.value.trim(),
      api: "",
      apiKey: apiKey.value.trim(),
      models: selectedModels.value,
    });
    emit("saved", summary);
  } catch (cause) {
    // A rejected draft keeps the form as typed: one field needs fixing.
    error.value = commandErrorMessage(cause);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <form
    class="space-y-3 rounded-lg border border-border p-3"
    @submit.prevent="handleSave"
  >
    <div class="flex items-start justify-between gap-3">
      <div class="min-w-0">
        <h4 class="text-sm font-medium">
          {{ t("settings.providers.custom.title") }}
        </h4>
        <p class="mt-0.5 text-xs text-muted-foreground">
          {{ t("settings.providers.custom.description") }}
        </p>
      </div>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        :disabled="saving"
        @click="emit('cancel')"
      >
        {{ t("common.cancel") }}
      </Button>
    </div>

    <!-- The preset sits above the fields it fills, so the order reads
         "pick an endpoint, then adjust it". -->
    <div class="space-y-1">
      <span id="provider-preset-label" class="text-xs font-medium">
        {{ t("settings.providers.custom.preset") }}
      </span>
      <Select
        :model-value="preset"
        @update:model-value="handlePresetChange(String($event))"
      >
        <SelectTrigger
          class="w-full"
          aria-labelledby="provider-preset-label"
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem
            v-for="entry in PROVIDER_PRESETS"
            :key="entry.value"
            :value="entry.value"
          >
            {{ entry.label }}
          </SelectItem>
          <SelectItem :value="CUSTOM_PRESET">
            {{ t("settings.providers.custom.preset.custom") }}
          </SelectItem>
        </SelectContent>
      </Select>
      <p class="text-xs text-muted-foreground">
        {{ t("settings.providers.custom.preset.description") }}
      </p>
    </div>

    <div class="grid gap-3 sm:grid-cols-2">
      <div class="space-y-1">
        <label for="provider-id" class="text-xs font-medium">
          {{ t("settings.providers.custom.id") }}
        </label>
        <Input
          id="provider-id"
          v-model="providerId"
          autocomplete="off"
          spellcheck="false"
          :placeholder="t('settings.providers.custom.idPlaceholder')"
        />
        <p class="text-xs text-muted-foreground">
          {{ t("settings.providers.custom.id.description") }}
        </p>
      </div>

      <div class="space-y-1">
        <label for="provider-name" class="text-xs font-medium">
          {{ t("settings.providers.custom.name") }}
        </label>
        <Input
          id="provider-name"
          v-model="providerName"
          autocomplete="off"
          :placeholder="t('settings.providers.custom.namePlaceholder')"
        />
        <p class="text-xs text-muted-foreground">
          {{ t("settings.providers.custom.name.description") }}
        </p>
      </div>
    </div>

    <div class="space-y-1">
      <label for="provider-base-url" class="text-xs font-medium">
        {{ t("settings.providers.custom.baseUrl") }}
      </label>
      <Input
        id="provider-base-url"
        v-model="baseUrl"
        autocomplete="off"
        spellcheck="false"
        :placeholder="t('settings.providers.custom.baseUrlPlaceholder')"
      />
      <p class="text-xs text-muted-foreground">
        {{ t("settings.providers.custom.baseUrl.description") }}
      </p>
    </div>

    <div class="space-y-1">
      <label for="provider-api-key" class="text-xs font-medium">
        {{ t("settings.providers.custom.apiKey") }}
      </label>
      <Input
        id="provider-api-key"
        v-model="apiKey"
        type="password"
        autocomplete="off"
        spellcheck="false"
        :placeholder="t('settings.providers.custom.apiKeyPlaceholder')"
      />
      <p class="text-xs text-muted-foreground">
        {{ t("settings.providers.custom.apiKey.description") }}
      </p>
    </div>

    <!-- S2: discovery is an explicit action, not a side effect of typing. -->
    <div class="space-y-2">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <h5 class="text-xs font-medium">
          {{ t("settings.providers.custom.models") }}
        </h5>
        <span
          v-if="found.length"
          class="text-xs text-muted-foreground"
        >
          {{
            t("settings.providers.custom.modelsSelected", {
              count: selectedModels.length,
              total: found.length,
            })
          }}
        </span>
      </div>

      <Button
        type="button"
        variant="outline"
        size="sm"
        :disabled="!canDiscover"
        @click="handleDiscover"
      >
        {{
          discovering
            ? t("settings.providers.custom.discovering")
            : t("settings.providers.custom.discover")
        }}
      </Button>

      <div
        v-if="found.length"
        class="max-h-44 space-y-0.5 overflow-y-auto rounded-md border border-border p-1"
      >
        <label
          v-for="model in found"
          :key="model.id"
          class="flex items-center gap-2 rounded-sm px-1.5 py-1 text-sm hover:bg-muted"
        >
          <input
            v-model="selected[model.id]"
            type="checkbox"
            class="size-4 shrink-0 accent-primary"
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
      <p
        v-else-if="!discovered"
        class="text-xs text-muted-foreground"
      >
        {{ t("settings.providers.custom.selectModels") }}
      </p>
    </div>

    <Alert v-if="error" variant="destructive">
      <AlertDescription>{{ error }}</AlertDescription>
    </Alert>

    <div class="flex flex-wrap items-center justify-between gap-2">
      <p
        v-if="!requiredFilled"
        class="text-xs text-muted-foreground"
      >
        {{ t("settings.providers.custom.required") }}
      </p>
      <span v-else />
      <Button type="submit" size="sm" :disabled="!canSave">
        {{
          saving
            ? t("settings.providers.custom.saving")
            : t("settings.providers.custom.save")
        }}
      </Button>
    </div>
  </form>
</template>
