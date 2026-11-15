<script setup lang="ts">
/**
 * Settings dialog (specs S1-S5 of `odd/tasks/settings-dialog.md`).
 *
 * Two panes, like a preferences window: a header with the title, the "open
 * configuration file" action and the close control; a left nav with the two
 * sections that have a real backend today (General, Models); and a scrolling
 * right pane of rows — label and one-line description on the left, control on
 * the right, sections separated by a subtle border.
 *
 * S5 is the reason the nav has exactly two entries: permissions, language, font
 * size, work details, code view and shortcuts have no backend yet, so they are
 * not painted as if they worked. They are listed as next units in the task doc.
 *
 * Preferences live in `settings.json`; this component owns the get/save flow
 * (`getSettings`/`saveSettings`) and applies the theme through the shared
 * `useAppearance().setMode`, the only path that touches the palette.
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
import { useAppearance } from "@/lib/appearance";
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

/** S1: the two sections the app can actually drive right now. */
type Section = "general" | "models";
const section = ref<Section>("general");

/** S3: the three modes, each with the icon its card shows. */
const THEME_OPTIONS: { mode: ThemeMode; label: string; icon: typeof SunIcon }[] = [
  { mode: "light", label: "Light", icon: SunIcon },
  { mode: "dark", label: "Dark", icon: MoonIcon },
  { mode: "system", label: "System", icon: MonitorIcon },
];

/** Persisted preferences, `null` until the file is read on open. */
const settings = ref<AppSettings | null>(null);
/** S4: model options come from `list_models`, never from a list kept here. */
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

/** S4: the persisted model id, `""` until the file is read. */
const modelId = computed(() => settings.value?.model ?? "");

/** S4: the provider is shown read-only; it is not editable here. */
const provider = computed(() => settings.value?.provider ?? "—");

async function loadPreferences() {
  loading.value = true;
  error.value = null;
  try {
    const loaded = await getSettings();
    settings.value = loaded;
    // S3: the file may have been edited outside the app, so the stored mode is
    // applied again on every open.
    setMode(loaded.theme);
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
 * S3: persists the appearance and applies it immediately. The whole file is
 * re-read first so this write cannot clobber a preference another surface
 * changed in the meantime (`save_settings` replaces the file).
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
 * S4: persists the chosen model. Same re-read-then-write rule as the theme; the
 * choice applies to the next session spawn.
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
 * S2: opens the real preferences file — `settings.json` — with the system's
 * default editor. Rust creates it with the defaults when it does not exist yet,
 * so the action never targets a missing path.
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
    <!-- The close control belongs to the header row (next to the open action),
         so the default floating X is off. -->
    <DialogContent
      :show-close-button="false"
      class="flex max-h-[min(85vh,44rem)] w-full max-w-2xl flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl"
    >
      <DialogHeader
        class="flex-row items-center justify-between gap-3 border-b border-border px-4 py-3"
      >
        <DialogTitle>Settings</DialogTitle>
        <div class="flex items-center gap-1.5">
          <Button
            variant="outline"
            size="sm"
            :disabled="opening"
            @click="handleOpenConfigFile"
          >
            <ExternalLinkIcon data-icon="inline-start" />
            Open configuration file
          </Button>
          <DialogClose as-child>
            <Button variant="ghost" size="icon-sm" aria-label="Close settings">
              <XIcon />
            </Button>
          </DialogClose>
        </div>
      </DialogHeader>

      <div class="flex min-h-0 flex-1">
        <!-- S1: left navigation. Active item uses the drawer's accent tokens so
             it reads as the same kind of selected row as a workspace. -->
        <nav
          aria-label="Settings sections"
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
            General
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
            Models
          </button>
        </nav>

        <!-- S1: the scrolling pane. One row per setting: label and description
             on the left, control on the right. -->
        <div class="min-h-0 flex-1 overflow-y-auto px-4 py-2">
          <div v-if="loading" class="py-6 text-sm text-muted-foreground">
            Reading settings…
          </div>

          <!-- S3: General — appearance, then the configuration file. -->
          <template v-else-if="section === 'general'">
            <section
              class="flex flex-wrap items-start justify-between gap-4 border-b border-border py-4"
            >
              <div class="min-w-0">
                <h3 class="text-sm font-medium">Appearance</h3>
                <p class="mt-0.5 text-xs text-muted-foreground">
                  Theme used across the whole app.
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
                  {{ option.label }}
                </button>
              </div>
            </section>

            <section class="flex flex-col gap-2 py-4">
              <div class="flex items-start justify-between gap-4">
                <div class="min-w-0">
                  <h3 class="text-sm font-medium">Configuration file</h3>
                  <p class="mt-0.5 text-xs text-muted-foreground">
                    Preferences are stored here; the system editor opens it.
                  </p>
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  :disabled="opening"
                  @click="handleOpenConfigFile"
                >
                  <ExternalLinkIcon data-icon="inline-start" />
                  Open
                </Button>
              </div>
              <!-- S3: the absolute path, resolved by Rust, read-only. -->
              <code
                class="block rounded-md bg-muted px-2.5 py-2 font-mono text-xs break-all text-muted-foreground select-all"
                >{{ configPath ?? "—" }}</code
              >
            </section>
          </template>

          <!-- S4: Models — the model selector and the provider it comes from. -->
          <template v-else>
            <section
              class="flex items-center justify-between gap-4 border-b border-border py-4"
            >
              <div class="min-w-0">
                <h3 class="text-sm font-medium">Model</h3>
                <p class="mt-0.5 text-xs text-muted-foreground">
                  Used by the next session the app spawns.
                </p>
              </div>
              <Select
                :model-value="modelId"
                :disabled="saving || models.length === 0"
                @update:model-value="handleModelChange(String($event))"
              >
                <SelectTrigger class="w-48" aria-label="Model">
                  <SelectValue placeholder="Select model" />
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

            <section class="flex items-center justify-between gap-4 py-4">
              <div class="min-w-0">
                <h3 class="text-sm font-medium">Provider</h3>
                <p class="mt-0.5 text-xs text-muted-foreground">
                  Read-only: the provider the app config names.
                </p>
              </div>
              <span class="text-sm text-muted-foreground">{{ provider }}</span>
            </section>

            <p class="pb-4 text-xs text-muted-foreground">
              The list comes from the provider configured in the app config
              today.
            </p>
          </template>

          <Alert v-if="error" variant="destructive" class="mb-4">
            <AlertDescription>{{ error }}</AlertDescription>
          </Alert>
        </div>
      </div>

      <DialogDescription class="sr-only">
        Preferences stored in the configuration file. General covers appearance
        and the configuration file itself; Models covers the model and provider.
      </DialogDescription>
    </DialogContent>
  </Dialog>
</template>
