<script setup lang="ts">
/**
 * Settings dialog (specs S1-S5 of `odd/tasks/settings-dialog.md`, S1/S2/S4 of
 * `odd/tasks/i18n-and-settings-polish.md`).
 *
 * Two panes, like a preferences window: a header with the title and the close
 * control; a left nav with the two sections that have a real backend today
 * (General, Models); and a scrolling right pane of rows — label and one-line
 * description on the left, control on the right.
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
  CpuIcon,
  ExternalLinkIcon,
  MonitorIcon,
  MoonIcon,
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
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
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
import {
  commandErrorMessage,
  configFilePath,
  getSettings,
  listModels,
  openConfigFile,
  saveSettings,
} from "@/contracts/commands";
import type { AppSettings, ModelInfo } from "@/contracts/types";

/** The dialog is open when the drawer's gear button says so. */
const open = defineModel<boolean>("open", { default: false });

/** S1 of `settings-dialog.md`: the two sections the app can drive right now. */
type Section = "general" | "models";
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
/** S4 of `settings-dialog.md`: model options come from `list_models`, never
 * from a list kept here. */
const models = ref<ModelInfo[]>([]);
/** S3: the absolute preferences path, resolved by Rust. */
const configPath = ref<string | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
/** A write is in flight, so the controls cannot fire twice. */
const saving = ref(false);
const opening = ref(false);

// The applied mode, not the stored one: the ring shows what the app is actually
// painting, and both agree because opening the dialog applies the stored mode.
const { mode, setMode } = useAppearance();

/** S4 of `settings-dialog.md`: the persisted model id, `""` until read. */
const modelId = computed(() => settings.value?.model ?? "");

/** S4 of `settings-dialog.md`: the provider is shown read-only. */
const provider = computed(() => settings.value?.provider ?? "—");

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

async function loadModels() {
  try {
    models.value = await listModels();
  } catch (cause) {
    // A missing model list must not break the dialog: the row still reports what
    // the file names and the message says why the options are absent.
    models.value = [];
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
 * S4 of `settings-dialog.md`: persists the chosen model. Same re-read-then-write
 * rule as the theme; the choice applies to the next session spawn.
 */
async function handleModelChange(value: string) {
  if (value === "" || saving.value) return;
  saving.value = true;
  error.value = null;
  try {
    const current = await getSettings();
    settings.value = await saveSettings({ ...current, model: value });
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    saving.value = false;
  }
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

// Every open re-reads the file, the model options and the path, so a human edit
// outside the app is picked up instead of showing a stale dialog.
watch(open, (isOpen) => {
  if (!isOpen) return;
  section.value = "general";
  error.value = null;
  void loadPreferences();
  void loadModels();
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
            :aria-current="section === 'models' ? 'page' : undefined"
            :class="[
              'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
              section === 'models'
                ? 'bg-sidebar-accent font-medium text-sidebar-accent-foreground'
                : 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground',
            ]"
            @click="section = 'models'"
          >
            <CpuIcon class="size-4 shrink-0" aria-hidden="true" />
            {{ t("settings.section.models") }}
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

            <!-- S1 first separator: Appearance | Configuration file. -->
            <Separator />

            <section class="flex flex-col gap-2">
              <div class="min-w-0">
                <h3 class="text-sm font-medium">
                  {{ t("settings.general.configurationFile") }}
                </h3>
                <p class="mt-0.5 text-xs text-muted-foreground">
                  {{ t("settings.general.configurationFile.description") }}
                </p>
              </div>
              <!-- S3 of `settings-dialog.md`: the absolute path, resolved by
                   Rust, read-only. -->
              <code
                class="block rounded-md bg-muted px-2.5 py-2 font-mono text-xs break-all text-muted-foreground select-all"
                >{{ configPath ?? "—" }}</code
              >
            </section>

            <!-- S1 second separator: Configuration file | Language. -->
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

          <!-- S4 of `settings-dialog.md`: Models — the model selector and the
               provider it comes from. -->
          <div v-else class="space-y-6 py-4">
            <section class="flex flex-wrap items-center justify-between gap-4">
              <div class="min-w-0">
                <h3 class="text-sm font-medium">{{ t("settings.models.model") }}</h3>
                <p class="mt-0.5 text-xs text-muted-foreground">
                  {{ t("settings.models.model.description") }}
                </p>
              </div>
              <Select
                :model-value="modelId"
                :disabled="saving || models.length === 0"
                @update:model-value="handleModelChange(String($event))"
              >
                <SelectTrigger
                  class="w-48"
                  :aria-label="t('settings.models.model')"
                >
                  <SelectValue
                    :placeholder="t('settings.models.model.selectPlaceholder')"
                  />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem
                    v-for="model in models"
                    :key="model.id"
                    :value="model.id"
                  >
                    {{ model.name }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </section>

            <section class="flex flex-wrap items-center justify-between gap-4">
              <div class="min-w-0">
                <h3 class="text-sm font-medium">
                  {{ t("settings.models.provider") }}
                </h3>
                <p class="mt-0.5 text-xs text-muted-foreground">
                  {{ t("settings.models.provider.description") }}
                </p>
              </div>
              <span class="text-sm text-muted-foreground">{{ provider }}</span>
            </section>

            <p class="text-xs text-muted-foreground">
              {{ t("settings.models.note") }}
            </p>
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
