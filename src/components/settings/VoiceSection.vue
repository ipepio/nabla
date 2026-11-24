<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import {
  AudioLinesIcon,
  CheckIcon,
  CpuIcon,
  DownloadIcon,
  Loader2Icon,
  MicIcon,
  PlusIcon,
  SquareIcon,
  Trash2Icon,
  Volume2Icon,
  XIcon,
} from "@lucide/vue";
import { speakText, speakingError, speakingKey, speakingLoading } from "@/lib/voice-player";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  commandErrorMessage,
  getSettings,
  saveSettings,
  subscribeVoiceDownloadProgress,
  voiceCancelDownload,
  voiceDeleteModel,
  voiceDownloadModel,
  voiceHostInfo,
  voiceImportCustomModel,
  voiceListModels,
} from "@/contracts/commands";
import type {
  AppSettings,
  HostHardwareInfo,
  VoiceDownloadProgress,
  VoiceModelEntry,
  VoiceModelKind,
  VoicePreferences,
} from "@/contracts/types";
import { t } from "@/lib/i18n";

const models = ref<VoiceModelEntry[]>([]);
const hostInfo = ref<HostHardwareInfo | null>(null);
const settings = ref<AppSettings | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);

const selectedSttFamily = ref<string>("");
const selectedTtsFamily = ref<string>("");
const selectedVoiceLang = ref<string>("all");

const activeSttEntry = computed(
  () => sttModels.value.find((m) => m.id === voicePrefs.value.sttModel) ?? null,
);
const activeTtsEntry = computed(
  () => ttsModels.value.find((m) => m.id === voicePrefs.value.ttsModel) ?? null,
);
const activeTtsVoiceLabel = computed(() => {
  const voice = activeTtsEntry.value?.voices.find(
    (v) => v.id === voicePrefs.value.ttsVoice,
  );
  return voice ? `${voice.name} (${voice.language})` : voicePrefs.value.ttsVoice || "—";
});

function voiceLangGroup(lang: string): string {
  return lang.toLowerCase().split("-")[0] || lang.toLowerCase();
}

function voiceLanguagesFor(model: VoiceModelEntry): string[] {
  const set = new Set<string>();
  for (const v of model.voices) {
    set.add(voiceLangGroup(v.language));
  }
  return [...set];
}

function filteredVoicesFor(model: VoiceModelEntry) {
  if (selectedVoiceLang.value === "all") return model.voices;
  return model.voices.filter(
    (v) => voiceLangGroup(v.language) === selectedVoiceLang.value,
  );
}

const progressByModel = ref<Record<string, VoiceDownloadProgress>>({});
let unlistenProgress: (() => void) | null = null;

// Custom ONNX model import form
const showImport = ref(false);
const importKind = ref<VoiceModelKind>("stt");
const importEngine = ref("moonshine-streaming");
const importName = ref("");
const importRepoOrUrl = ref("");
const importLanguage = ref("es");
const importing = ref(false);

const defaultVoicePrefs: VoicePreferences = {
  sttModel: "",
  ttsModel: "",
  ttsVoice: "",
  ttsSpeed: 1.0,
  autoRead: false,
};

const voicePrefs = computed<VoicePreferences>(
  () => settings.value?.voice ?? defaultVoicePrefs,
);

const sttModels = computed(() => models.value.filter((m) => m.kind === "stt"));
const ttsModels = computed(() => models.value.filter((m) => m.kind === "tts"));

interface FamilyGroup {
  family: string;
  recommended: boolean;
  hasInstalled: boolean;
  items: VoiceModelEntry[];
}

function groupByFamily(list: VoiceModelEntry[]): FamilyGroup[] {
  const map = new Map<string, FamilyGroup>();
  for (const item of list) {
    const key = item.family || item.name;
    const existing = map.get(key);
    if (existing) {
      existing.items.push(item);
      if (item.recommended) existing.recommended = true;
      if (item.status === "installed") existing.hasInstalled = true;
    } else {
      map.set(key, {
        family: key,
        recommended: item.recommended,
        hasInstalled: item.status === "installed",
        items: [item],
      });
    }
  }
  return [...map.values()];
}

const sttFamilies = computed(() => groupByFamily(sttModels.value));
const ttsFamilies = computed(() => groupByFamily(ttsModels.value));

const activeSttGroup = computed<FamilyGroup | null>(() => {
  return (
    sttFamilies.value.find((g) => g.family === selectedSttFamily.value) ??
    sttFamilies.value[0] ??
    null
  );
});

const activeTtsGroup = computed<FamilyGroup | null>(() => {
  return (
    ttsFamilies.value.find((g) => g.family === selectedTtsFamily.value) ??
    ttsFamilies.value[0] ??
    null
  );
});

function syncDefaultFamilySelection(): void {
  if (!selectedSttFamily.value || !sttFamilies.value.some((g) => g.family === selectedSttFamily.value)) {
    const active = sttModels.value.find((m) => m.id === voicePrefs.value.sttModel);
    const rec = sttFamilies.value.find((g) => g.recommended);
    selectedSttFamily.value = active?.family ?? rec?.family ?? sttFamilies.value[0]?.family ?? "";
  }
  if (!selectedTtsFamily.value || !ttsFamilies.value.some((g) => g.family === selectedTtsFamily.value)) {
    const active = ttsModels.value.find((m) => m.id === voicePrefs.value.ttsModel);
    const rec = ttsFamilies.value.find((g) => g.recommended);
    selectedTtsFamily.value = active?.family ?? rec?.family ?? ttsFamilies.value[0]?.family ?? "";
  }
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "—";
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function progressPercent(modelId: string): number {
  const p = progressByModel.value[modelId];
  if (!p || p.totalBytes <= 0) return 0;
  return Math.min(100, Math.max(0, Math.round((p.downloadedBytes / p.totalBytes) * 100)));
}

async function refresh(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    const [list, current, host] = await Promise.all([
      voiceListModels(),
      getSettings(),
      voiceHostInfo(),
    ]);
    models.value = list;
    settings.value = current;
    hostInfo.value = host;
    syncDefaultFamilySelection();
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    loading.value = false;
  }
}

async function patchVoicePrefs(patch: Partial<VoicePreferences>): Promise<void> {
  if (!settings.value) return;
  const next: AppSettings = {
    ...settings.value,
    voice: {
      ...voicePrefs.value,
      ...patch,
    },
  };
  try {
    settings.value = await saveSettings(next);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  }
}

async function handleDownload(model: VoiceModelEntry, voiceIds?: string[]): Promise<void> {
  error.value = null;
  try {
    const defaultVoices =
      !voiceIds && model.status === "not_installed" && model.voices.length > 0
        ? [model.voices[0].id]
        : voiceIds;
    await voiceDownloadModel(model.id, defaultVoices);
    models.value = await voiceListModels();
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  }
}

async function handleCancel(modelId: string): Promise<void> {
  error.value = null;
  try {
    await voiceCancelDownload(modelId);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  }
}

async function handleDelete(model: VoiceModelEntry, voiceId?: string): Promise<void> {
  error.value = null;
  try {
    await voiceDeleteModel(model.id, voiceId);
    models.value = await voiceListModels();
    syncDefaultFamilySelection();
    if (!voiceId) {
      if (model.kind === "stt" && voicePrefs.value.sttModel === model.id) {
        await patchVoicePrefs({ sttModel: "" });
      }
      if (model.kind === "tts" && voicePrefs.value.ttsModel === model.id) {
        await patchVoicePrefs({ ttsModel: "", ttsVoice: "" });
      }
    } else if (voicePrefs.value.ttsVoice === voiceId) {
      await patchVoicePrefs({ ttsVoice: "" });
    }
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  }
}

async function handleSelectStt(model: VoiceModelEntry): Promise<void> {
  await patchVoicePrefs({ sttModel: model.id });
}

function previewVoice(model: VoiceModelEntry, voiceId: string, language: string): void {
  const sample = language.toLowerCase().startsWith("es")
    ? "Hola, esta es una prueba de voz local ejecutándose en CPU con Nabla."
    : "Hello, this is a local CPU voice preview running inside Nabla.";
  void speakText(`preview:${model.id}:${voiceId}`, sample, {
    modelId: model.id,
    voiceId,
    speed: voicePrefs.value.ttsSpeed,
    language,
  });
}

async function handleSelectTts(model: VoiceModelEntry, voiceId?: string): Promise<void> {
  const chosenVoice =
    voiceId ??
    model.voices.find((v) => v.installed)?.id ??
    voicePrefs.value.ttsVoice;
  await patchVoicePrefs({ ttsModel: model.id, ttsVoice: chosenVoice });
}

function onImportKindChange(kind: VoiceModelKind): void {
  importKind.value = kind;
  importEngine.value = kind === "tts" ? "kokoro" : "moonshine-streaming";
}

async function handleImport(): Promise<void> {
  if (!importName.value.trim() || !importRepoOrUrl.value.trim() || importing.value) return;
  importing.value = true;
  error.value = null;
  try {
    const imported = await voiceImportCustomModel({
      kind: importKind.value,
      engine: importEngine.value,
      name: importName.value.trim(),
      repoOrUrl: importRepoOrUrl.value.trim(),
      language: importLanguage.value.trim() || undefined,
    });
    importName.value = "";
    importRepoOrUrl.value = "";
    showImport.value = false;
    models.value = await voiceListModels();
    if (imported.kind === "stt") {
      selectedSttFamily.value = imported.family;
    } else {
      selectedTtsFamily.value = imported.family;
    }
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    importing.value = false;
  }
}

onMounted(async () => {
  await refresh();
  unlistenProgress = await subscribeVoiceDownloadProgress(async (progress) => {
    if (progress.status === "downloading") {
      progressByModel.value = {
        ...progressByModel.value,
        [progress.modelId]: progress,
      };
      return;
    }
    const next = { ...progressByModel.value };
    delete next[progress.modelId];
    progressByModel.value = next;

    if (progress.status === "error" && progress.error) {
      error.value = progress.error;
    }
    try {
      models.value = await voiceListModels();
      if (progress.status === "completed") {
        const installed = models.value.find((m) => m.id === progress.modelId);
        if (installed?.kind === "stt" && installed.runtimeReady && !voicePrefs.value.sttModel) {
          await patchVoicePrefs({ sttModel: installed.id });
        } else if (
          installed?.kind === "tts" &&
          installed.runtimeReady &&
          !voicePrefs.value.ttsModel
        ) {
          const firstVoice = installed.voices.find((v) => v.installed)?.id ?? "";
          await patchVoicePrefs({ ttsModel: installed.id, ttsVoice: firstVoice });
        }
      }
    } catch {
      // Best effort refresh.
    }
  });
});

onUnmounted(() => {
  unlistenProgress?.();
  unlistenProgress = null;
});
</script>

<template>
  <div class="space-y-6">
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div class="space-y-1">
        <h3 class="flex items-center gap-2 text-sm font-semibold text-foreground">
          <AudioLinesIcon class="size-4 text-primary" aria-hidden="true" />
          {{ t("settings.voice.title") }}
        </h3>
        <p class="text-xs text-muted-foreground">
          {{ t("settings.voice.description") }}
        </p>
        <div
          v-if="hostInfo"
          class="inline-flex items-center gap-1.5 rounded-md border border-border bg-secondary/50 px-2 py-0.5 text-[10.5px] text-muted-foreground tabular-nums"
        >
          <CpuIcon class="size-3 text-primary" aria-hidden="true" />
          <span>
            {{ hostInfo.cpuCores }} CPU · {{ hostInfo.totalRamGb }} GB RAM · {{ hostInfo.arch }}
          </span>
        </div>
      </div>
      <Button
        size="sm"
        variant="outline"
        class="h-8 gap-1.5 text-xs"
        @click="showImport = !showImport"
      >
        <PlusIcon class="size-3.5" aria-hidden="true" />
        {{ t("settings.voice.importCustom") }}
      </Button>
    </div>

    <!-- Import custom ONNX model form -->
    <form
      v-if="showImport"
      class="space-y-3 rounded-xl border border-border bg-secondary/40 p-4"
      @submit.prevent="handleImport"
    >
      <div class="flex items-center justify-between">
        <span class="text-xs font-semibold text-foreground">
          {{ t("settings.voice.importTitle") }}
        </span>
        <button
          type="button"
          class="rounded-md p-1 text-muted-foreground hover:bg-secondary hover:text-foreground"
          :aria-label="t('common.close')"
          @click="showImport = false"
        >
          <XIcon class="size-3.5" />
        </button>
      </div>

      <div class="grid grid-cols-1 gap-2.5 sm:grid-cols-2">
        <div class="space-y-1">
          <label class="text-[11px] font-medium text-muted-foreground">
            {{ t("settings.voice.importKind") }}
          </label>
          <div class="flex gap-1.5">
            <Button
              type="button"
              size="sm"
              :variant="importKind === 'stt' ? 'default' : 'outline'"
              class="h-7 flex-1 text-xs"
              @click="onImportKindChange('stt')"
            >
              STT
            </Button>
            <Button
              type="button"
              size="sm"
              :variant="importKind === 'tts' ? 'default' : 'outline'"
              class="h-7 flex-1 text-xs"
              @click="onImportKindChange('tts')"
            >
              TTS
            </Button>
          </div>
        </div>

        <div class="space-y-1">
          <label class="text-[11px] font-medium text-muted-foreground">
            {{ t("settings.voice.importEngine") }}
          </label>
          <select
            v-model="importEngine"
            class="h-7 w-full rounded-md border border-input bg-background px-2 text-xs text-foreground"
          >
            <option v-if="importKind === 'stt'" value="moonshine-streaming">
              Moonshine Streaming (ONNX)
            </option>
            <option v-if="importKind === 'stt'" value="moonshine-seq2seq">
              Moonshine Seq2Seq (ONNX)
            </option>
            <option v-if="importKind === 'stt'" value="whisper">
              Whisper (ONNX)
            </option>
            <option v-if="importKind === 'stt'" value="parakeet">
              Parakeet TDT (ONNX)
            </option>
            <option v-if="importKind === 'tts'" value="kokoro">
              Kokoro (ONNX)
            </option>
            <option v-if="importKind === 'tts'" value="phonon-gguf">
              Phonon (GGUF)
            </option>
          </select>
        </div>

        <div class="space-y-1">
          <label class="text-[11px] font-medium text-muted-foreground">
            {{ t("common.name") }}
          </label>
          <Input
            v-model="importName"
            type="text"
            class="h-8 text-xs"
            :placeholder="t('settings.voice.importNamePlaceholder')"
          />
        </div>

        <div class="space-y-1">
          <label class="text-[11px] font-medium text-muted-foreground">
            {{ t("settings.voice.importLanguage") }}
          </label>
          <Input
            v-model="importLanguage"
            type="text"
            class="h-8 text-xs"
            placeholder="es, en"
          />
        </div>
      </div>

      <div class="space-y-1">
        <label class="text-[11px] font-medium text-muted-foreground">
          {{ t("settings.voice.importRepo") }}
        </label>
        <Input
          v-model="importRepoOrUrl"
          type="text"
          class="h-8 text-xs"
          placeholder="owner/repo or https://huggingface.co/owner/repo"
        />
      </div>

      <div class="flex justify-end gap-2">
        <Button type="button" size="sm" variant="outline" @click="showImport = false">
          {{ t("common.cancel") }}
        </Button>
        <Button
          type="submit"
          size="sm"
          :disabled="importing || !importName.trim() || !importRepoOrUrl.trim()"
        >
          {{ t("settings.voice.importSubmit") }}
        </Button>
      </div>
    </form>

    <Alert v-if="error || speakingError" variant="destructive">
      <AlertDescription>{{ error || speakingError }}</AlertDescription>
    </Alert>

    <!-- Active engines summary bar -->
    <div
      v-if="settings"
      class="grid grid-cols-1 gap-2 rounded-xl border border-border bg-secondary/35 p-3 sm:grid-cols-2"
    >
      <div class="flex items-center justify-between gap-2 rounded-lg bg-card px-3 py-2">
        <div class="min-w-0">
          <div class="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
            {{ t("settings.voice.activeStt") }}
          </div>
          <div class="truncate text-xs font-semibold text-foreground">
            {{ activeSttEntry ? activeSttEntry.name : t("settings.voice.noneActive") }}
          </div>
        </div>
        <Button
          v-if="activeSttEntry"
          size="sm"
          variant="ghost"
          class="h-6 px-2 text-[11px] text-muted-foreground hover:text-foreground"
          @click="patchVoicePrefs({ sttModel: '' })"
        >
          {{ t("settings.voice.disable") }}
        </Button>
      </div>

      <div class="flex items-center justify-between gap-2 rounded-lg bg-card px-3 py-2">
        <div class="min-w-0">
          <div class="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
            {{ t("settings.voice.activeTts") }}
          </div>
          <div class="truncate text-xs font-semibold text-foreground">
            <template v-if="activeTtsEntry">
              {{ activeTtsEntry.name }} · {{ activeTtsVoiceLabel }}
            </template>
            <template v-else>
              {{ t("settings.voice.noneActive") }}
            </template>
          </div>
        </div>
        <Button
          v-if="activeTtsEntry"
          size="sm"
          variant="ghost"
          class="h-6 px-2 text-[11px] text-muted-foreground hover:text-foreground"
          @click="patchVoicePrefs({ ttsModel: '', ttsVoice: '' })"
        >
          {{ t("settings.voice.disable") }}
        </Button>
      </div>
    </div>

    <div v-if="loading && models.length === 0" class="py-4 text-xs text-muted-foreground">
      {{ t("common.loading") }}
    </div>

    <!-- Speech-to-Text (STT): Model family selector -> sizes/variants below -->
    <section v-if="sttFamilies.length > 0" class="space-y-3 rounded-xl border border-border bg-background/60 p-4">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="flex items-center gap-2">
          <MicIcon class="size-4 text-primary" aria-hidden="true" />
          <h4 class="text-xs font-semibold uppercase tracking-wide text-foreground">
            {{ t("settings.voice.sttHeading") }}
          </h4>
        </div>
        <select
          v-model="selectedSttFamily"
          :aria-label="t('settings.voice.modelSelector')"
          class="h-8 min-w-[220px] rounded-lg border border-input bg-card px-2.5 text-xs font-medium text-foreground"
        >
          <option
            v-for="group in sttFamilies"
            :key="group.family"
            :value="group.family"
          >
            {{ group.family }}{{ group.recommended ? ` (${t("settings.voice.recommended")})` : "" }}
          </option>
        </select>
      </div>

      <div v-if="activeSttGroup" class="space-y-2 border-t border-border pt-3">
        <div class="text-[10.5px] font-medium text-muted-foreground">
          {{ t("settings.voice.sizesHeading") }}
        </div>
        <div
          v-for="model in activeSttGroup.items"
          :key="model.id"
          class="rounded-lg border border-border/80 bg-card p-3"
        >
          <div class="flex flex-wrap items-start justify-between gap-2">
            <div class="min-w-0 flex-1 space-y-1">
              <div class="flex flex-wrap items-center gap-1.5">
                <span class="text-xs font-semibold text-foreground">{{ model.variantLabel }}</span>
                <span
                  v-if="model.recommended"
                  class="rounded bg-primary/15 px-1.5 py-0.5 text-[10px] font-medium text-primary"
                >
                  {{ t("settings.voice.recommended") }}
                </span>
                <span
                  v-if="!model.runtimeReady"
                  class="rounded bg-secondary px-1.5 py-0.5 text-[10px] text-muted-foreground"
                >
                  {{ t("settings.voice.comingSoon") }}
                </span>
                <span class="rounded bg-secondary px-1.5 py-0.5 text-[10px] text-muted-foreground">
                  {{ model.languages.join(", ").toUpperCase() }}
                </span>
                <span class="rounded bg-secondary px-1.5 py-0.5 text-[10px] text-muted-foreground">
                  {{ model.license }}
                </span>
                <span
                  v-if="model.minRamGb > 0"
                  class="rounded px-1.5 py-0.5 text-[10px] tabular-nums"
                  :class="
                    hostInfo && model.minRamGb > hostInfo.totalRamGb
                      ? 'bg-destructive/15 text-destructive'
                      : 'bg-secondary text-muted-foreground'
                  "
                >
                  {{ model.minRamGb }} GB RAM+
                </span>
                <span class="text-[10px] text-muted-foreground tabular-nums">
                  {{
                    model.installedBytes > 0
                      ? formatBytes(model.installedBytes)
                      : formatBytes(model.approxBytes)
                  }}
                </span>
              </div>
              <p class="text-[11px] text-muted-foreground">{{ model.description }}</p>
            </div>

            <div class="flex shrink-0 items-center gap-1.5">
              <template v-if="model.status === 'downloading'">
                <Button
                  size="sm"
                  variant="outline"
                  class="h-7 gap-1 text-xs"
                  @click="handleCancel(model.id)"
                >
                  <XIcon class="size-3" />
                  {{ t("common.cancel") }}
                </Button>
              </template>
              <template v-else-if="model.status === 'installed'">
                <Button
                  size="sm"
                  :variant="voicePrefs.sttModel === model.id ? 'default' : 'outline'"
                  :disabled="!model.runtimeReady"
                  class="h-7 gap-1 text-xs"
                  @click="handleSelectStt(model)"
                >
                  <CheckIcon v-if="voicePrefs.sttModel === model.id" class="size-3" />
                  {{
                    voicePrefs.sttModel === model.id
                      ? t("settings.voice.active")
                      : t("settings.voice.use")
                  }}
                </Button>
                <Button
                  size="icon-xs"
                  variant="ghost"
                  class="text-muted-foreground hover:bg-destructive/15 hover:text-destructive"
                  :title="t('common.delete')"
                  :aria-label="t('common.delete')"
                  @click="handleDelete(model)"
                >
                  <Trash2Icon class="size-3.5" />
                </Button>
              </template>
              <template v-else>
                <Button
                  size="sm"
                  variant="outline"
                  class="h-7 gap-1.5 text-xs"
                  @click="handleDownload(model)"
                >
                  <DownloadIcon class="size-3" />
                  {{ t("settings.voice.download") }}
                </Button>
                <Button
                  v-if="model.custom"
                  size="icon-xs"
                  variant="ghost"
                  class="text-muted-foreground hover:bg-destructive/15 hover:text-destructive"
                  :title="t('common.delete')"
                  @click="handleDelete(model)"
                >
                  <Trash2Icon class="size-3.5" />
                </Button>
              </template>
            </div>
          </div>

          <div v-if="model.status === 'downloading'" class="mt-2.5 space-y-1">
            <div class="h-1.5 w-full overflow-hidden rounded-full bg-secondary">
              <div
                class="h-full rounded-full bg-primary transition-[width] duration-150"
                :style="{ width: `${progressPercent(model.id)}%` }"
              />
            </div>
            <div class="flex items-center justify-between text-[10px] text-muted-foreground tabular-nums">
              <span class="truncate">{{ progressByModel[model.id]?.fileName || t("settings.voice.downloading") }}</span>
              <span>
                {{ formatBytes(progressByModel[model.id]?.downloadedBytes ?? 0) }}
                /
                {{ formatBytes(progressByModel[model.id]?.totalBytes ?? model.approxBytes) }}
                ({{ progressPercent(model.id) }}%)
              </span>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- Text-to-Speech (TTS): Model selector -> model status & voices below -->
    <section v-if="ttsFamilies.length > 0" class="space-y-3 rounded-xl border border-border bg-background/60 p-4">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="flex items-center gap-2">
          <Volume2Icon class="size-4 text-primary" aria-hidden="true" />
          <h4 class="text-xs font-semibold uppercase tracking-wide text-foreground">
            {{ t("settings.voice.ttsHeading") }}
          </h4>
        </div>
        <div class="flex flex-wrap items-center gap-2">
          <div class="flex items-center gap-1.5 text-[11px] text-muted-foreground">
            <span>{{ t("settings.voice.speed") }}</span>
            <select
              :value="voicePrefs.ttsSpeed"
              class="h-8 rounded-lg border border-input bg-card px-2 text-xs text-foreground tabular-nums"
              @change="patchVoicePrefs({ ttsSpeed: Number(($event.target as HTMLSelectElement).value) })"
            >
              <option :value="0.85">0.85x</option>
              <option :value="1">1.0x</option>
              <option :value="1.15">1.15x</option>
              <option :value="1.3">1.3x</option>
            </select>
          </div>
          <select
            v-model="selectedTtsFamily"
            :aria-label="t('settings.voice.modelSelector')"
            class="h-8 min-w-[200px] rounded-lg border border-input bg-card px-2.5 text-xs font-medium text-foreground"
            @change="selectedVoiceLang = 'all'"
          >
            <option
              v-for="group in ttsFamilies"
              :key="group.family"
              :value="group.family"
            >
              {{ group.family }}{{ group.recommended ? ` (${t("settings.voice.recommended")})` : "" }}
            </option>
          </select>
        </div>
      </div>

      <div v-if="activeTtsGroup" class="space-y-3 border-t border-border pt-3">
        <div
          v-for="model in activeTtsGroup.items"
          :key="model.id"
          class="space-y-3"
        >
          <div class="flex flex-wrap items-start justify-between gap-2">
            <div class="min-w-0 flex-1 space-y-1">
              <div class="flex flex-wrap items-center gap-1.5">
                <span class="text-xs font-semibold text-foreground">{{ model.name }}</span>
                <span
                  v-if="model.recommended"
                  class="rounded bg-primary/15 px-1.5 py-0.5 text-[10px] font-medium text-primary"
                >
                  {{ t("settings.voice.recommended") }}
                </span>
                <span
                  v-if="!model.runtimeReady"
                  class="rounded bg-secondary px-1.5 py-0.5 text-[10px] text-muted-foreground"
                >
                  {{ t("settings.voice.comingSoon") }}
                </span>
                <span class="rounded bg-secondary px-1.5 py-0.5 text-[10px] text-muted-foreground">
                  {{ model.params }}
                </span>
                <span class="rounded bg-secondary px-1.5 py-0.5 text-[10px] text-muted-foreground">
                  {{ model.languages.join(", ").toUpperCase() }}
                </span>
                <span class="rounded bg-secondary px-1.5 py-0.5 text-[10px] text-muted-foreground">
                  {{ model.license }}
                </span>
                <span
                  v-if="model.minRamGb > 0"
                  class="rounded px-1.5 py-0.5 text-[10px] tabular-nums"
                  :class="
                    hostInfo && model.minRamGb > hostInfo.totalRamGb
                      ? 'bg-destructive/15 text-destructive'
                      : 'bg-secondary text-muted-foreground'
                  "
                >
                  {{ model.minRamGb }} GB RAM+
                </span>
                <span class="text-[10px] text-muted-foreground tabular-nums">
                  {{
                    model.installedBytes > 0
                      ? formatBytes(model.installedBytes)
                      : formatBytes(model.approxBytes)
                  }}
                </span>
              </div>
              <p class="text-[11px] text-muted-foreground">{{ model.description }}</p>
            </div>

            <div class="flex shrink-0 items-center gap-1.5">
              <template v-if="model.status === 'downloading'">
                <Button
                  size="sm"
                  variant="outline"
                  class="h-7 gap-1 text-xs"
                  @click="handleCancel(model.id)"
                >
                  <XIcon class="size-3" />
                  {{ t("common.cancel") }}
                </Button>
              </template>
              <template v-else-if="model.status === 'installed'">
                <Button
                  size="sm"
                  :variant="voicePrefs.ttsModel === model.id ? 'default' : 'outline'"
                  :disabled="!model.runtimeReady"
                  class="h-7 gap-1 text-xs"
                  @click="handleSelectTts(model)"
                >
                  <CheckIcon v-if="voicePrefs.ttsModel === model.id" class="size-3" />
                  {{
                    voicePrefs.ttsModel === model.id
                      ? t("settings.voice.active")
                      : t("settings.voice.use")
                  }}
                </Button>
                <Button
                  size="icon-xs"
                  variant="ghost"
                  class="text-muted-foreground hover:bg-destructive/15 hover:text-destructive"
                  :title="t('common.delete')"
                  :aria-label="t('common.delete')"
                  @click="handleDelete(model)"
                >
                  <Trash2Icon class="size-3.5" />
                </Button>
              </template>
              <template v-else>
                <Button
                  size="sm"
                  variant="outline"
                  class="h-7 gap-1.5 text-xs"
                  @click="handleDownload(model)"
                >
                  <DownloadIcon class="size-3" />
                  {{ t("settings.voice.download") }}
                </Button>
              </template>
            </div>
          </div>

          <div v-if="model.status === 'downloading'" class="space-y-1">
            <div class="h-1.5 w-full overflow-hidden rounded-full bg-secondary">
              <div
                class="h-full rounded-full bg-primary transition-[width] duration-150"
                :style="{ width: `${progressPercent(model.id)}%` }"
              />
            </div>
            <div class="flex items-center justify-between text-[10px] text-muted-foreground tabular-nums">
              <span class="truncate">{{ progressByModel[model.id]?.fileName || t("settings.voice.downloading") }}</span>
              <span>
                {{ formatBytes(progressByModel[model.id]?.downloadedBytes ?? 0) }}
                /
                {{ formatBytes(progressByModel[model.id]?.totalBytes ?? model.approxBytes) }}
                ({{ progressPercent(model.id) }}%)
              </span>
            </div>
          </div>

          <!-- Voices for the selected TTS model -->
          <div v-if="model.voices.length > 0" class="space-y-2 border-t border-border/70 pt-2.5">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div class="text-[10.5px] font-medium text-muted-foreground">
                {{ t("settings.voice.voicesHeading") }} ({{ filteredVoicesFor(model).length }} / {{ model.voices.length }})
              </div>
              <div class="flex flex-wrap items-center gap-1">
                <button
                  type="button"
                  class="rounded-md px-2 py-0.5 text-[10px] font-medium transition-colors"
                  :class="
                    selectedVoiceLang === 'all'
                      ? 'bg-primary text-primary-foreground'
                      : 'bg-secondary text-muted-foreground hover:text-foreground'
                  "
                  @click="selectedVoiceLang = 'all'"
                >
                  {{ t("settings.voice.allLanguages") }}
                </button>
                <button
                  v-for="lang in voiceLanguagesFor(model)"
                  :key="lang"
                  type="button"
                  class="rounded-md px-2 py-0.5 text-[10px] font-medium uppercase transition-colors"
                  :class="
                    selectedVoiceLang === lang
                      ? 'bg-primary text-primary-foreground'
                      : 'bg-secondary text-muted-foreground hover:text-foreground'
                  "
                  @click="selectedVoiceLang = lang"
                >
                  {{ lang }}
                </button>
              </div>
            </div>
            <div class="grid max-h-64 grid-cols-1 gap-1.5 overflow-y-auto pr-1 sm:grid-cols-2">
              <div
                v-for="voice in filteredVoicesFor(model)"
                :key="voice.id"
                class="flex items-center justify-between gap-2 rounded-lg border border-border/70 bg-card px-2.5 py-1.5"
              >
                <div class="min-w-0">
                  <div class="flex items-center gap-1.5">
                    <span class="truncate text-xs font-medium text-foreground">{{ voice.name }}</span>
                    <span class="rounded bg-secondary px-1 py-0.5 text-[9.5px] uppercase text-muted-foreground">
                      {{ voice.language }}
                    </span>
                  </div>
                  <div class="text-[10px] text-muted-foreground">
                    {{ voice.id }} · {{ formatBytes(voice.installed ? voice.installedBytes : voice.approxBytes) }}
                  </div>
                </div>

                <div class="flex shrink-0 items-center gap-1">
                  <template v-if="voice.installed">
                    <Button
                      v-if="model.runtimeReady"
                      size="icon-xs"
                      variant="ghost"
                      class="size-6 text-muted-foreground hover:text-foreground"
                      :disabled="model.status !== 'installed'"
                      :title="t('settings.voice.preview')"
                      :aria-label="t('settings.voice.preview')"
                      @click="previewVoice(model, voice.id, voice.language)"
                    >
                      <Loader2Icon
                        v-if="speakingKey === `preview:${model.id}:${voice.id}` && speakingLoading"
                        class="size-3 animate-spin"
                      />
                      <SquareIcon
                        v-else-if="speakingKey === `preview:${model.id}:${voice.id}`"
                        class="size-2.5 fill-current text-primary"
                      />
                      <Volume2Icon v-else class="size-3" />
                    </Button>
                    <Button
                      size="sm"
                      :disabled="!model.runtimeReady"
                      :variant="
                        voicePrefs.ttsModel === model.id && voicePrefs.ttsVoice === voice.id
                          ? 'default'
                          : 'outline'
                      "
                      class="h-6 px-2 text-[11px]"
                      @click="handleSelectTts(model, voice.id)"
                    >
                      {{
                        voicePrefs.ttsModel === model.id && voicePrefs.ttsVoice === voice.id
                          ? t("settings.voice.active")
                          : t("settings.voice.use")
                      }}
                    </Button>
                    <Button
                      size="icon-xs"
                      variant="ghost"
                      class="size-6 text-muted-foreground hover:bg-destructive/15 hover:text-destructive"
                      :title="t('common.delete')"
                      @click="handleDelete(model, voice.id)"
                    >
                      <Trash2Icon class="size-3" />
                    </Button>
                  </template>
                  <template v-else>
                    <Button
                      size="sm"
                      variant="outline"
                      class="h-6 gap-1 px-2 text-[11px]"
                      :disabled="model.status === 'downloading'"
                      @click="handleDownload(model, [voice.id])"
                    >
                      <DownloadIcon class="size-3" />
                      {{ t("settings.voice.download") }}
                    </Button>
                  </template>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  </div>
</template>
