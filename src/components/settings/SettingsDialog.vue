<script setup lang="ts">
/**
 * Settings dialog (specs S1-S5 of `odd/tasks/settings-dialog.md`, S1/S2/S4 of
 * `odd/tasks/i18n-and-settings-polish.md`).
 *
 * Two panes, like a preferences window: a header with the title and the close
 * control; a left nav with the two sections that have a real backend today
 * (General, Providers); and a scrolling right pane of rows — label and one-line
 * description on the left, control on the right.
 *
 * S1 of `odd/tasks/providers-pane.md`: the second pane is Providers, not Models.
 * It lists the providers pi knows and marks the active one (`settings.provider`);
 * clicking a row persists that choice. The model selector moved out of this
 * dialog: the chat composer owns it and already writes `settings.model`.
 *
 * S5 of the same task: the custom-provider form is gated by
 * `VITE_ENABLE_CUSTOM_PROVIDERS`, a build-time switch like the rest of Nabla's
 * env configuration — and it is ON BY DEFAULT: only an explicit `0`/`false`/`off`
 * hides the block. The same helper declares `VITE_ENABLE_PROVIDER_LOGIN` and
 * `VITE_ENABLE_API_KEYS`, which render nothing yet: those two sections belong to
 * the next unit, so this build has no dead control.
 *
 * S3/S4: every configured row also offers the api key of that provider (typed
 * inline, never prefilled, written into pi's plain-text `models.json`) and its
 * removal behind a confirmation. Removing the active provider falls back to the
 * first remaining one; with none left the settings stay untouched and the pane
 * says so.
 *
 * S1 of `i18n-and-settings-polish.md`: the pane has no rule between single rows.
 * Blocks are separated by spacing plus at most two rules, one between each pair
 * of major blocks (Appearance / Configuration file / Language), so the pane reads
 * as three groups instead of a wall of lines.
 *
 * S4: the configuration-file action exists exactly once, at the end of the
 * General pane; the header no longer repeats it.
 *
 * S5 of `settings-dialog.md`: the nav has exactly two entries — permissions,
 * font size, work details, code view and shortcuts have no backend yet, so they
 * are not painted as if they worked.
 *
 * Preferences live in `settings.json`; this component owns the get/save flow
 * (`getSettings`/`saveSettings`) and applies the theme through the shared
 * `useAppearance().setMode`, the only path that touches the palette. Every
 * user-visible string comes from the catalogue in `src/lib/i18n.ts` (S2).
 */
import { computed, ref, watch } from "vue";
import {
  CheckIcon,
  ExternalLinkIcon,
  MonitorIcon,
  MoonIcon,
  PlugIcon,
  PlusIcon,
  SettingsIcon,
  SunIcon,
  XIcon,
} from "@lucide/vue";
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
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { useAppearance } from "@/lib/appearance";
import { useI18n, type Locale, type MessageKey } from "@/lib/i18n";
import type { ThemeMode } from "@/lib/theme";
import ProviderForm from "./ProviderForm.vue";
import {
  commandErrorMessage,
  configFilePath,
  deleteProvider,
  getSettings,
  listProviders,
  openConfigFile,
  saveSettings,
  setProviderKey,
} from "@/contracts/commands";
import type { AppSettings, ProviderSummary } from "@/contracts/types";

/** The dialog is open when the drawer's gear button says so. */
const open = defineModel<boolean>("open", { default: false });

/** S1 of `settings-dialog.md`: the two sections the app can drive right now. */
type Section = "general" | "providers";
const section = ref<Section>("general");

/** S2: every row renders through the catalogue, so a locale switch re-renders. */
const { locale, setLocale, t } = useI18n();

/** S3: the three modes, each with the icon its card shows. */
const THEME_OPTIONS: {
  mode: ThemeMode;
  labelKey: MessageKey;
  icon: typeof SunIcon;
}[] = [
  {
    mode: "light",
    labelKey: "settings.general.appearance.light",
    icon: SunIcon,
  },
  { mode: "dark", labelKey: "settings.general.appearance.dark", icon: MoonIcon },
  {
    mode: "system",
    labelKey: "settings.general.appearance.system",
    icon: MonitorIcon,
  },
];

/**
 * S2: the locales the selector offers. The label is each language's own name, so
 * someone who cannot read the current interface language still finds their own.
 */
const LANGUAGE_OPTIONS: { value: Locale; labelKey: MessageKey }[] = [
  { value: "en", labelKey: "settings.general.language.english" },
  { value: "es", labelKey: "settings.general.language.spanish" },
];

/** Persisted preferences, `null` until the file is read on open. */
const settings = ref<AppSettings | null>(null);
/** S1 of `providers-pane.md`: the providers pi knows, as Rust summarizes them. */
const providers = ref<ProviderSummary[]>([]);
/** S3: the absolute preferences path, resolved by Rust. */
const configPath = ref<string | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
/** A write is in flight, so the controls cannot fire twice. */
const saving = ref(false);
const opening = ref(false);
/** S2: the inline custom-provider form is open. */
const showProviderForm = ref(false);
/** S4: the provider whose inline api-key editor is open, `null` when closed. */
const keyTarget = ref<string | null>(null);
/** The key being typed. Always empty when the editor opens: never prefilled. */
const keyDraft = ref("");
/** A key write is in flight. */
const savingKey = ref(false);
/** The provider the removal confirmation is about, `null` when it is closed. */
const pendingRemoval = ref<ProviderSummary | null>(null);
const removalOpen = ref(false);
/** A removal finished, so a write is in flight. */
const removing = ref(false);
/**
 * A line the pane shows after a removal that left the settings untouched (S1
 * fallback could not happen). Informational, not an error: nothing failed.
 */
const warning = ref<string | null>(null);

/**
 * S5 of `providers-pane.md`: build-time switches, read the same way the palette
 * and the hero copy are. ON BY DEFAULT — only an explicit `0`/`false`/`off`
 * (any case, surrounding spaces ignored) turns a section off, so a build without
 * these variables configured shows the features instead of hiding them.
 */
function readFlag(name: string): boolean {
  const value = (import.meta.env as Record<string, string | undefined>)[name];
  const normalized = (value ?? "").trim().toLowerCase();
  return normalized !== "0" && normalized !== "false" && normalized !== "off";
}

/**
 * S5 of `providers-pane.md`: build-time switch, read the same way the palette and
 * the hero copy are. The custom-provider block is the only gated section of this
 * pane; the pi login flow was dropped from scope, so no flag is declared for it
 * (a switch that renders nothing is dead configuration).
 */
const customProvidersEnabled = readFlag("VITE_ENABLE_CUSTOM_PROVIDERS");

// The applied mode, not the stored one: the ring shows what the app is actually
// painting, and both agree because opening the dialog applies the stored mode.
const { mode, setMode } = useAppearance();

/** S1 of `providers-pane.md`: the provider the next session starts with. */
const activeProvider = computed(() => settings.value?.provider ?? "");

async function loadPreferences() {
  loading.value = true;
  error.value = null;
  try {
    const loaded = await getSettings();
    settings.value = loaded;
    // S3 of `settings-dialog.md`: the file may have been edited outside the app,
    // so the stored mode is applied again on every open. S2 of
    // `i18n-and-settings-polish.md`: same rule for the language.
    setMode(loaded.theme);
    setLocale(loaded.language);
  } catch (cause) {
    settings.value = null;
    error.value = commandErrorMessage(cause);
  } finally {
    loading.value = false;
  }
}

/**
 * S1 of `providers-pane.md`: the list is read from pi's own configuration on
 * every open, so a provider added elsewhere shows up instead of going stale.
 */
async function loadProviders() {
  try {
    providers.value = await listProviders();
  } catch (cause) {
    providers.value = [];
    error.value = commandErrorMessage(cause);
  }
}

async function loadConfigPath() {
  try {
    configPath.value = await configFilePath();
  } catch (cause) {
    configPath.value = null;
    error.value = commandErrorMessage(cause);
  }
}

/**
 * S3 of `settings-dialog.md`: persists the appearance and applies it
 * immediately. The whole file is re-read first so this write cannot clobber a
 * preference another surface changed in the meantime (`save_settings` replaces
 * the file).
 */
async function handleThemeChange(next: ThemeMode) {
  if (saving.value || mode.value === next) return;
  saving.value = true;
  error.value = null;
  try {
    const current = await getSettings();
    settings.value = await saveSettings({ ...current, theme: next });
    setMode(next);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    saving.value = false;
  }
}

/**
 * S1 of `providers-pane.md`: clicking a provider makes it the active one by
 * persisting `settings.provider`.
 *
 * `settings.model` is left exactly as it is: this pane has no catalogue to check
 * it against, and the chat composer owns that choice (L4 of the task doc).
 */
async function handleProviderActivate(providerId: string) {
  if (saving.value || providerId === activeProvider.value) return;
  saving.value = true;
  error.value = null;
  try {
    const current = await getSettings();
    settings.value = await saveSettings({ ...current, provider: providerId });
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    saving.value = false;
  }
}

/**
 * S4: opens the inline api-key editor for one provider. The field always starts
 * empty — Rust never sends the key, so there is nothing to prefill.
 */
function toggleKeyEditor(entry: ProviderSummary) {
  if (keyTarget.value === entry.id) {
    closeKeyEditor();
    return;
  }
  keyTarget.value = entry.id;
  keyDraft.value = "";
  error.value = null;
}

function closeKeyEditor() {
  keyTarget.value = null;
  keyDraft.value = "";
}

/**
 * S4: stores the typed key through `set_provider_key`, which rewrites
 * `providers.<id>.apiKey` in pi's plain-text file (backup + atomic write). Rust
 * answers with the summary the file now holds, so the row is refreshed from it
 * instead of guessing, and "key set" appears because the file says so.
 *
 * An empty field cannot be submitted (the button is disabled), and the typed key
 * is dropped once the write lands.
 */
async function handleSaveKey() {
  const target = keyTarget.value;
  const key = keyDraft.value.trim();
  if (!target || key === "" || savingKey.value) return;
  savingKey.value = true;
  error.value = null;
  try {
    const summary = await setProviderKey(target, key);
    providers.value = providers.value.map((entry) =>
      entry.id === summary.id ? summary : entry,
    );
    closeKeyEditor();
  } catch (cause) {
    // A rejected key keeps the editor open with what was typed.
    error.value = commandErrorMessage(cause);
  } finally {
    savingKey.value = false;
  }
}

/** Opens the removal confirmation for one provider (S1/S3). */
function requestRemove(entry: ProviderSummary) {
  pendingRemoval.value = entry;
  removalOpen.value = true;
  error.value = null;
  warning.value = null;
}

/**
 * S3: removes one provider from pi's configuration after confirmation, then
 * re-reads the list (Rust may have had nothing to remove).
 *
 * S1 fallback: if the removed provider was the active one, the first remaining
 * provider becomes active; with none left, the settings are left untouched and
 * the pane says so instead of writing a provider the file no longer has.
 */
async function confirmRemove() {
  const target = pendingRemoval.value;
  if (!target || removing.value) return;
  removalOpen.value = false;
  pendingRemoval.value = null;
  removing.value = true;
  error.value = null;
  warning.value = null;
  try {
    // An editor left open on the removed row would have no provider to write.
    if (keyTarget.value === target.id) closeKeyEditor();
    const removed = await deleteProvider(target.id);
    await loadProviders();
    if (!removed) {
      // The entry was already gone from the file: report it instead of
      // pretending the removal happened.
      warning.value = t("settings.providers.remove.missing");
      return;
    }
    if (target.id !== activeProvider.value) return;
    const remaining = providers.value[0];
    if (!remaining) {
      warning.value = t("settings.providers.remove.activeWarning");
      return;
    }
    const current = await getSettings();
    settings.value = await saveSettings({
      ...current,
      provider: remaining.id,
    });
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    removing.value = false;
  }
}

/**
 * S2 of `providers-pane.md`: a saved provider lands in pi's file, so the list is
 * re-read instead of patched locally — Rust is the one that knows the stored
 * name and model count (an empty name falls back to the id there).
 */
function handleProviderSaved(_summary: ProviderSummary) {
  showProviderForm.value = false;
  void loadProviders();
}

/**
 * S2 of `i18n-and-settings-polish.md`: persists the language and applies it at
 * once, so the dialog itself switches without a reload. The write lands first:
 * a failed save must not leave the UI in a language the file does not record.
 */
async function handleLanguageChange(next: string) {
  if (saving.value) return;
  saving.value = true;
  error.value = null;
  try {
    const current = await getSettings();
    settings.value = await saveSettings({ ...current, language: next });
    setLocale(next);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    saving.value = false;
  }
}

/**
 * S2 of `settings-dialog.md`: opens the real preferences file — `settings.json` —
 * with the system's default editor. Rust creates it with the defaults when it
 * does not exist yet, so the action never targets a missing path. S4 of
 * `i18n-and-settings-polish.md` keeps this as the only such action.
 */
async function handleOpenConfigFile() {
  if (opening.value) return;
  opening.value = true;
  error.value = null;
  try {
    await openConfigFile();
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    opening.value = false;
  }
}

// Every open re-reads the file, the provider list and the path, so a human edit
// outside the app is picked up instead of showing a stale dialog. The inline form
// starts closed, the way a preferences window does.
watch(open, (isOpen) => {
  if (!isOpen) return;
  section.value = "general";
  showProviderForm.value = false;
  closeKeyEditor();
  pendingRemoval.value = null;
  removalOpen.value = false;
  savingKey.value = false;
  removing.value = false;
  warning.value = null;
  error.value = null;
  void loadPreferences();
  void loadProviders();
  void loadConfigPath();
});
</script>

<template>
  <Dialog v-model:open="open">
    <!-- The close control is the only header action (S4: the configuration-file
         action lives at the end of the General pane), so the default floating X
         is off. -->
    <DialogContent
      :show-close-button="false"
      class="flex max-h-[min(85vh,44rem)] w-full max-w-2xl flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl"
    >
      <DialogHeader class="flex-row items-center justify-between gap-3 px-4 py-3">
        <DialogTitle>{{ t("settings.title") }}</DialogTitle>
        <DialogClose as-child>
          <Button
            variant="ghost"
            size="icon-sm"
            :aria-label="t('settings.close')"
          >
            <XIcon />
          </Button>
        </DialogClose>
      </DialogHeader>

      <div class="flex min-h-0 flex-1">
        <!-- S1: left navigation. Active item uses the drawer's accent tokens so
             it reads as the same kind of selected row as a workspace. -->
        <nav
          :aria-label="t('settings.sections')"
          class="flex w-44 shrink-0 flex-col gap-0.5 border-r border-border p-2"
        >
          <button
            type="button"
            :aria-current="section === 'general' ? 'page' : undefined"
            :class="[
              'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
              section === 'general'
                ? 'bg-sidebar-accent font-medium text-sidebar-accent-foreground'
                : 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground',
            ]"
            @click="section = 'general'"
          >
            <SettingsIcon class="size-4 shrink-0" aria-hidden="true" />
            {{ t("settings.section.general") }}
          </button>
          <button
            type="button"
            :aria-current="section === 'providers' ? 'page' : undefined"
            :class="[
              'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
              section === 'providers'
                ? 'bg-sidebar-accent font-medium text-sidebar-accent-foreground'
                : 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground',
            ]"
            @click="section = 'providers'"
          >
            <PlugIcon class="size-4 shrink-0" aria-hidden="true" />
            {{ t("settings.section.providers") }}
          </button>
        </nav>

        <!-- S1: the scrolling pane. One row per setting: label and description
             on the left, control on the right. Blocks are spaced, not ruled: the
             only separators are the two between Appearance / Configuration file /
             Language. -->
        <div class="min-h-0 flex-1 overflow-y-auto px-4 py-2">
          <div v-if="loading" class="py-6 text-sm text-muted-foreground">
            {{ t("settings.loading") }}
          </div>

          <!-- S3 of `settings-dialog.md`: General — appearance, the configuration
               file and the language (S2 of `i18n-and-settings-polish.md`). -->
          <div v-else-if="section === 'general'" class="space-y-6 py-4">
            <section class="flex flex-wrap items-start justify-between gap-4">
              <div class="min-w-0">
                <h3 class="text-sm font-medium">
                  {{ t("settings.general.appearance") }}
                </h3>
                <p class="mt-0.5 text-xs text-muted-foreground">
                  {{ t("settings.general.appearance.description") }}
                </p>
              </div>
              <div class="flex gap-2">
                <button
                  v-for="option in THEME_OPTIONS"
                  :key="option.mode"
                  type="button"
                  :aria-pressed="mode === option.mode"
                  :disabled="saving"
                  :class="[
                    'flex w-16 flex-col items-center gap-1.5 rounded-lg border border-border py-2.5 text-xs outline-hidden transition-colors hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50',
                    mode === option.mode
                      ? 'bg-muted font-medium text-foreground ring-2 ring-ring'
                      : 'text-muted-foreground',
                  ]"
                  @click="handleThemeChange(option.mode)"
                >
                  <component :is="option.icon" class="size-4" aria-hidden="true" />
                  {{ t(option.labelKey) }}
                </button>
              </div>
            </section>

            <!-- One separator only: Appearance | Language. The configuration
                 block that used to sit here was redundant with the single
                 "open configuration file" action at the end of the pane. -->
            <Separator />

            <section class="flex flex-wrap items-center justify-between gap-4">
              <div class="min-w-0">
                <h3 class="text-sm font-medium">
                  {{ t("settings.general.language") }}
                </h3>
                <p class="mt-0.5 text-xs text-muted-foreground">
                  {{ t("settings.general.language.description") }}
                </p>
              </div>
              <Select
                :model-value="locale"
                :disabled="saving"
                @update:model-value="handleLanguageChange(String($event))"
              >
                <SelectTrigger
                  class="w-40"
                  :aria-label="t('settings.general.language')"
                >
                  <SelectValue
                    :placeholder="t('settings.general.language.selectPlaceholder')"
                  />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem
                    v-for="option in LANGUAGE_OPTIONS"
                    :key="option.value"
                    :value="option.value"
                  >
                    {{ t(option.labelKey) }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </section>

            <!-- S4: the only configuration-file action, at the end of the pane. -->
            <div class="flex justify-end">
              <Button
                variant="outline"
                size="sm"
                :disabled="opening"
                @click="handleOpenConfigFile"
              >
                <ExternalLinkIcon data-icon="inline-start" />
                {{ t("settings.general.configurationFile.open") }}
              </Button>
            </div>
          </div>

          <!-- S1 of `providers-pane.md`: Providers — every provider pi knows, with
               the active one marked. Rows carry no rule of their own: the only
               separator in this pane is the one before the custom block. -->
          <div v-else class="space-y-6 py-4">
            <section class="space-y-2">
              <div class="min-w-0">
                <h3 class="text-sm font-medium">
                  {{ t("settings.providers.title") }}
                </h3>
                <p class="mt-0.5 text-xs text-muted-foreground">
                  {{ t("settings.providers.description") }}
                </p>
              </div>

              <!-- S1: the name, url and state are one activate control; the row's
                   other actions are siblings of it (a button inside a button is
                   invalid HTML). The active row is marked by the accent fill, not
                   by a rule: this pane keeps a single separator, before the
                   custom block. -->
              <ul v-if="providers.length" class="space-y-0.5">
                <li
                  v-for="entry in providers"
                  :key="entry.id"
                  :class="[
                    'rounded-md transition-colors',
                    entry.id === activeProvider
                      ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                      : 'hover:bg-muted',
                  ]"
                >
                  <div class="flex items-center gap-1 px-2 py-1.5">
                    <button
                      type="button"
                      :aria-pressed="entry.id === activeProvider"
                      :disabled="saving || removing"
                      class="min-w-0 flex-1 rounded-sm text-left outline-hidden focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
                      @click="handleProviderActivate(entry.id)"
                    >
                      <span class="flex items-center justify-between gap-2">
                        <span class="flex min-w-0 items-baseline gap-2">
                          <span class="min-w-0 truncate text-sm font-medium">
                            {{ entry.name }}
                          </span>
                          <span
                            class="min-w-0 truncate font-mono text-xs text-muted-foreground"
                          >
                            {{ entry.id }}
                          </span>
                        </span>
                        <CheckIcon
                          v-if="entry.id === activeProvider"
                          class="size-4 shrink-0"
                          aria-hidden="true"
                        />
                      </span>
                      <span
                        class="mt-0.5 flex items-center justify-between gap-2 text-xs text-muted-foreground"
                      >
                        <span class="min-w-0 truncate" :title="entry.baseUrl">
                          {{ entry.baseUrl }}
                        </span>
                        <span class="shrink-0">
                          {{
                            t("settings.providers.models", {
                              count: entry.modelCount,
                            })
                          }}
                          ·
                          {{
                            entry.hasApiKey
                              ? t("settings.providers.keySet")
                              : t("settings.providers.noKey")
                          }}
                        </span>
                      </span>
                    </button>

                    <!-- S4: the api-key action of this row. -->
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      :aria-expanded="keyTarget === entry.id"
                      :disabled="removing"
                      @click="toggleKeyEditor(entry)"
                    >
                      {{ t("settings.providers.apiKey") }}
                    </Button>
                    <!-- S1/S3: removal, behind the pane's confirmation dialog. -->
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      :disabled="removing"
                      @click="requestRemove(entry)"
                    >
                      {{ t("settings.providers.remove") }}
                    </Button>
                  </div>

                  <!-- S4: the key is typed here and never prefilled — Rust has
                       no copy to send back, so the field always starts empty. -->
                  <form
                    v-if="keyTarget === entry.id"
                    class="flex flex-wrap items-center gap-2 px-2 pb-2"
                    @submit.prevent="handleSaveKey"
                  >
                    <Input
                      v-model="keyDraft"
                      type="password"
                      autocomplete="new-password"
                      spellcheck="false"
                      class="h-8 min-w-0 flex-1 text-sm"
                      :aria-label="
                        t('settings.providers.apiKey.label', { id: entry.id })
                      "
                      :placeholder="t('settings.providers.apiKey.placeholder')"
                    />
                    <Button
                      type="submit"
                      size="sm"
                      :disabled="!keyDraft.trim() || savingKey"
                    >
                      {{
                        savingKey
                          ? t("settings.providers.apiKey.saving")
                          : t("settings.providers.apiKey.save")
                      }}
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      :disabled="savingKey"
                      @click="closeKeyEditor"
                    >
                      {{ t("common.cancel") }}
                    </Button>
                    <p class="w-full text-xs text-muted-foreground">
                      {{ t("settings.providers.apiKey.description") }}
                    </p>
                  </form>
                </li>
              </ul>
              <p v-else class="text-xs text-muted-foreground">
                {{ t("settings.providers.empty") }}
              </p>

              <!-- Informational line after a removal: nothing failed, so it is
                   not rendered as an error. -->
              <p v-if="warning" class="text-xs text-muted-foreground">
                {{ warning }}
              </p>
            </section>

            <!-- S5: this whole block is ON unless the build sets
                 VITE_ENABLE_CUSTOM_PROVIDERS to `0`/`false`/`off`; with the flag
                 off there is nothing to click and nothing to explain. -->
            <template v-if="customProvidersEnabled">
              <Separator />

              <section class="space-y-3">
                <Button
                  v-if="!showProviderForm"
                  type="button"
                  variant="outline"
                  size="sm"
                  @click="showProviderForm = true"
                >
                  <PlusIcon data-icon="inline-start" />
                  {{ t("settings.providers.custom.title") }}
                </Button>
                <ProviderForm
                  v-else
                  @saved="handleProviderSaved"
                  @cancel="showProviderForm = false"
                />
              </section>
            </template>

            <!-- S1/S3: one confirmation dialog for the whole list, the same
                 pattern the session rows use: the trigger only says which
                 provider it is about. -->
            <Dialog v-model:open="removalOpen">
              <DialogContent>
                <DialogHeader>
                  <DialogTitle>
                    {{ t("settings.providers.remove.title") }}
                  </DialogTitle>
                  <DialogDescription>
                    {{
                      t("settings.providers.remove.description", {
                        id: pendingRemoval?.id ?? "",
                      })
                    }}
                  </DialogDescription>
                </DialogHeader>
                <DialogFooter>
                  <DialogClose as-child>
                    <Button variant="outline">
                      {{ t("common.cancel") }}
                    </Button>
                  </DialogClose>
                  <Button
                    variant="destructive"
                    :disabled="removing"
                    @click="confirmRemove"
                  >
                    {{ t("settings.providers.remove") }}
                  </Button>
                </DialogFooter>
              </DialogContent>
            </Dialog>
          </div>

          <Alert v-if="error" variant="destructive" class="mb-4">
            <AlertDescription>{{ error }}</AlertDescription>
          </Alert>
        </div>
      </div>

      <DialogDescription class="sr-only">
        {{ t("settings.description") }}
      </DialogDescription>
    </DialogContent>
  </Dialog>
</template>
