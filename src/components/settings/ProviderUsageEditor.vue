<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { GaugeIcon, LoaderCircleIcon, Trash2Icon } from "@lucide/vue";
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
import {
  commandErrorMessage,
  deleteProviderUsageConfig,
  listProviderUsageConfigs,
  saveProviderUsageConfig,
  testProviderUsageConfig,
} from "@/contracts/commands";
import type {
  ProviderQuotaSnapshot,
  ProviderUsageConfigDraft,
} from "@/contracts/types";

const props = defineProps<{
  providerId: string;
  providerName: string;
  baseUrl?: string | null;
}>();

type PresetChoice = "nan" | "openrouter" | "deepseek" | "custom";

const expanded = ref(false);
const loading = ref(false);
const saving = ref(false);
const testing = ref(false);
const saved = ref(false);
const hasStoredConfig = ref(false);
const hasSecret = ref(false);
const error = ref<string | null>(null);
const testResult = ref<ProviderQuotaSnapshot | null>(null);

const presetChoice = ref<PresetChoice>("custom");
const enabled = ref(true);
const url = ref("");
const method = ref<"GET" | "POST">("GET");
const authMode = ref<"providerApiKey" | "cookie" | "bearer" | "none">("providerApiKey");
const secretDraft = ref("");
const origin = ref("");
const referer = ref("");
const windowLabel = ref("");
const usedPath = ref("");
const remainingPath = ref("");
const limitPath = ref("");
const resetAtPath = ref("");
const planPath = ref("");
const unit = ref<"percent" | "usd" | "credits" | "tokens">("percent");

const isNanPreset = computed(() => presetChoice.value === "nan");

function applyPresetDefaults(choice: PresetChoice): void {
  presetChoice.value = choice;
  testResult.value = null;
  error.value = null;
  if (choice === "nan") {
    url.value = "https://cloud-api.nan.builders/api/usage/credits";
    method.value = "GET";
    authMode.value = "cookie";
    origin.value = "https://cloud.nan.builders";
    referer.value = "https://cloud.nan.builders/";
    windowLabel.value = "Créditos";
    usedPath.value = "";
    remainingPath.value = "";
    limitPath.value = "";
    resetAtPath.value = "";
    planPath.value = "";
    unit.value = "credits";
  } else if (choice === "openrouter") {
    url.value = "https://openrouter.ai/api/v1/auth/key";
    method.value = "GET";
    authMode.value = "providerApiKey";
    origin.value = "";
    referer.value = "";
    windowLabel.value = "Credits";
    usedPath.value = "data.usage";
    remainingPath.value = "";
    limitPath.value = "data.limit";
    resetAtPath.value = "";
    planPath.value = "data.label";
    unit.value = "usd";
  } else if (choice === "deepseek") {
    url.value = "https://api.deepseek.com/user/balance";
    method.value = "GET";
    authMode.value = "providerApiKey";
    origin.value = "";
    referer.value = "";
    windowLabel.value = "Balance";
    usedPath.value = "";
    remainingPath.value = "balance_infos.0.total_balance";
    limitPath.value = "";
    resetAtPath.value = "";
    planPath.value = "";
    unit.value = "usd";
  }
}

function guessInitialPreset(): PresetChoice {
  const id = props.providerId.toLowerCase();
  const base = (props.baseUrl ?? "").toLowerCase();
  if (id === "nan" || base.includes("nan.builders")) return "nan";
  if (id.includes("openrouter") || base.includes("openrouter.ai")) return "openrouter";
  if (id.includes("deepseek") || base.includes("deepseek.com")) return "deepseek";
  return "custom";
}

async function loadExisting(): Promise<void> {
  loading.value = true;
  error.value = null;
  testResult.value = null;
  saved.value = false;
  secretDraft.value = "";
  try {
    const list = await listProviderUsageConfigs();
    const existing = list.find((c) => c.providerId === props.providerId);
    if (existing) {
      hasStoredConfig.value = true;
      expanded.value = true;
      enabled.value = existing.enabled;
      hasSecret.value = existing.hasSecret;
      url.value = existing.url;
      method.value = existing.method === "POST" ? "POST" : "GET";
      authMode.value =
        existing.authMode === "cookie" ||
        existing.authMode === "bearer" ||
        existing.authMode === "none"
          ? existing.authMode
          : "providerApiKey";
      origin.value = existing.origin ?? "";
      referer.value = existing.referer ?? "";
      windowLabel.value = existing.windowLabel ?? "";
      usedPath.value = existing.usedPath ?? "";
      remainingPath.value = existing.remainingPath ?? "";
      limitPath.value = existing.limitPath ?? "";
      resetAtPath.value = existing.resetAtPath ?? "";
      planPath.value = existing.planPath ?? "";
      unit.value =
        existing.unit === "usd" || existing.unit === "credits" || existing.unit === "tokens"
          ? existing.unit
          : "percent";
      if (existing.preset === "nan") {
        presetChoice.value = "nan";
      } else if (existing.url.includes("openrouter.ai")) {
        presetChoice.value = "openrouter";
      } else if (existing.url.includes("deepseek.com")) {
        presetChoice.value = "deepseek";
      } else {
        presetChoice.value = "custom";
      }
    } else {
      hasStoredConfig.value = false;
      hasSecret.value = false;
      enabled.value = true;
      const guessed = guessInitialPreset();
      applyPresetDefaults(guessed);
    }
  } catch (err) {
    error.value = commandErrorMessage(err);
  } finally {
    loading.value = false;
  }
}

function buildDraft(): ProviderUsageConfigDraft {
  return {
    providerId: props.providerId,
    name: props.providerName,
    enabled: enabled.value,
    preset: isNanPreset.value ? "nan" : "declarative",
    url: url.value.trim(),
    method: method.value,
    authMode: isNanPreset.value ? "cookie" : authMode.value,
    secret: secretDraft.value.trim() !== "" ? secretDraft.value.trim() : null,
    clearSecret: false,
    origin: origin.value.trim() || null,
    referer: referer.value.trim() || null,
    windowLabel: windowLabel.value.trim() || null,
    usedPath: usedPath.value.trim() || null,
    remainingPath: remainingPath.value.trim() || null,
    limitPath: limitPath.value.trim() || null,
    resetAtPath: resetAtPath.value.trim() || null,
    planPath: planPath.value.trim() || null,
    unit: unit.value,
  };
}

async function handleTest(): Promise<void> {
  testing.value = true;
  error.value = null;
  testResult.value = null;
  try {
    testResult.value = await testProviderUsageConfig(buildDraft());
  } catch (err) {
    error.value = commandErrorMessage(err);
  } finally {
    testing.value = false;
  }
}

async function handleSave(): Promise<void> {
  saving.value = true;
  error.value = null;
  saved.value = false;
  try {
    const summary = await saveProviderUsageConfig(buildDraft());
    hasStoredConfig.value = true;
    hasSecret.value = summary.hasSecret;
    secretDraft.value = "";
    saved.value = true;
  } catch (err) {
    error.value = commandErrorMessage(err);
  } finally {
    saving.value = false;
  }
}

async function handleDelete(): Promise<void> {
  saving.value = true;
  error.value = null;
  try {
    await deleteProviderUsageConfig(props.providerId);
    hasStoredConfig.value = false;
    hasSecret.value = false;
    secretDraft.value = "";
    testResult.value = null;
    saved.value = false;
    expanded.value = false;
  } catch (err) {
    error.value = commandErrorMessage(err);
  } finally {
    saving.value = false;
  }
}

onMounted(() => {
  void loadExisting();
});

watch(
  () => props.providerId,
  () => {
    void loadExisting();
  },
);
</script>

<template>
  <section class="space-y-2.5">
    <div class="flex items-center justify-between gap-2">
      <div class="flex items-center gap-1.5">
        <GaugeIcon class="size-3.5 text-muted-foreground" />
        <h4 class="text-xs font-semibold text-foreground">Usage &amp; Quota Endpoint</h4>
        <span
          v-if="hasStoredConfig"
          class="rounded-full bg-primary/15 px-1.5 py-0.5 text-[10px] font-medium text-primary"
        >
          Configured
        </span>
      </div>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        class="h-7 px-2 text-xs"
        @click="expanded = !expanded"
      >
        {{ expanded ? "Hide" : hasStoredConfig ? "Edit" : "Configure" }}
      </Button>
    </div>

    <p v-if="!expanded" class="text-[11px] text-muted-foreground">
      Optionally configure a declarative HTTPS endpoint (no scripts executed) to show live quota in the usage popover.
    </p>

    <div v-else class="space-y-3 rounded-lg border border-border bg-secondary/25 p-3">
      <div class="grid gap-2.5 sm:grid-cols-2">
        <div class="space-y-1">
          <label class="text-[11px] font-medium text-foreground">Preset</label>
          <Select
            :model-value="presetChoice"
            @update:model-value="applyPresetDefaults(String($event) as PresetChoice)"
          >
            <SelectTrigger class="h-8 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="nan">NaN Builders (Credits &amp; Quota)</SelectItem>
              <SelectItem value="openrouter">OpenRouter (API Key)</SelectItem>
              <SelectItem value="deepseek">DeepSeek (API Key)</SelectItem>
              <SelectItem value="custom">Custom HTTPS / JSON</SelectItem>
            </SelectContent>
          </Select>
        </div>

        <div v-if="!isNanPreset" class="space-y-1">
          <label class="text-[11px] font-medium text-foreground">Authentication</label>
          <Select
            :model-value="authMode"
            @update:model-value="authMode = String($event) as typeof authMode"
          >
            <SelectTrigger class="h-8 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="providerApiKey">Provider API key (Bearer)</SelectItem>
              <SelectItem value="cookie">Session Cookie header</SelectItem>
              <SelectItem value="bearer">Custom Bearer token</SelectItem>
              <SelectItem value="none">None</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      <!-- NaN Builders cookie instructions -->
      <div v-if="isNanPreset" class="space-y-1.5">
        <div class="flex items-center justify-between">
          <label class="text-[11px] font-medium text-foreground">
            Session Cookie (cloud.nan.builders)
          </label>
          <span v-if="hasSecret" class="text-[10px] font-medium text-primary">
            Cookie saved
          </span>
        </div>
        <Input
          v-model="secretDraft"
          type="password"
          autocomplete="off"
          spellcheck="false"
          class="h-8 font-mono text-xs"
          :placeholder="hasSecret ? 'Leave blank to keep saved cookie' : 'Paste Cookie header from DevTools → Network'"
        />
        <p class="text-[10.5px] text-muted-foreground">
          Queries <span class="font-mono">/api/usage/credits</span> (with fallback to
          <span class="font-mono">/api/usage/quota</span> and <span class="font-mono">/api/billing</span>) directly from Rust.
        </p>
      </div>

      <!-- Custom / declarative fields -->
      <template v-else>
        <div class="grid gap-2 sm:grid-cols-[1fr_5.5rem]">
          <div class="space-y-1">
            <label class="text-[11px] font-medium text-foreground">HTTPS URL</label>
            <Input
              v-model="url"
              autocomplete="off"
              spellcheck="false"
              class="h-8 font-mono text-xs"
              placeholder="https://api.example.com/v1/usage"
            />
          </div>
          <div class="space-y-1">
            <label class="text-[11px] font-medium text-foreground">Method</label>
            <Select
              :model-value="method"
              @update:model-value="method = String($event) === 'POST' ? 'POST' : 'GET'"
            >
              <SelectTrigger class="h-8 text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="GET">GET</SelectItem>
                <SelectItem value="POST">POST</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>

        <div v-if="authMode === 'cookie' || authMode === 'bearer'" class="space-y-1">
          <div class="flex items-center justify-between">
            <label class="text-[11px] font-medium text-foreground">
              {{ authMode === "cookie" ? "Cookie header" : "Bearer token" }}
            </label>
            <span v-if="hasSecret" class="text-[10px] font-medium text-primary">
              Saved
            </span>
          </div>
          <Input
            v-model="secretDraft"
            type="password"
            autocomplete="off"
            spellcheck="false"
            class="h-8 font-mono text-xs"
            :placeholder="hasSecret ? 'Leave blank to keep saved credential' : 'Paste secret credential'"
          />
        </div>

        <div class="grid gap-2 sm:grid-cols-3">
          <div class="space-y-1">
            <label class="text-[11px] font-medium text-foreground">Used JSON path</label>
            <Input
              v-model="usedPath"
              spellcheck="false"
              class="h-8 font-mono text-xs"
              placeholder="data.usage"
            />
          </div>
          <div class="space-y-1">
            <label class="text-[11px] font-medium text-foreground">Remaining path</label>
            <Input
              v-model="remainingPath"
              spellcheck="false"
              class="h-8 font-mono text-xs"
              placeholder="data.remaining"
            />
          </div>
          <div class="space-y-1">
            <label class="text-[11px] font-medium text-foreground">Limit JSON path</label>
            <Input
              v-model="limitPath"
              spellcheck="false"
              class="h-8 font-mono text-xs"
              placeholder="data.limit"
            />
          </div>
        </div>

        <div class="grid gap-2 sm:grid-cols-3">
          <div class="space-y-1">
            <label class="text-[11px] font-medium text-foreground">Unit</label>
            <Select
              :model-value="unit"
              @update:model-value="unit = String($event) as typeof unit"
            >
              <SelectTrigger class="h-8 text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="percent">Percent (%)</SelectItem>
                <SelectItem value="usd">USD ($)</SelectItem>
                <SelectItem value="credits">Credits</SelectItem>
                <SelectItem value="tokens">Tokens</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div class="space-y-1">
            <label class="text-[11px] font-medium text-foreground">Label</label>
            <Input
              v-model="windowLabel"
              class="h-8 text-xs"
              placeholder="Usage"
            />
          </div>
          <div class="space-y-1">
            <label class="text-[11px] font-medium text-foreground">Reset path (opt)</label>
            <Input
              v-model="resetAtPath"
              spellcheck="false"
              class="h-8 font-mono text-xs"
              placeholder="data.reset_at"
            />
          </div>
        </div>
      </template>

      <Alert v-if="error" variant="destructive">
        <AlertDescription class="text-xs">{{ error }}</AlertDescription>
      </Alert>

      <div
        v-if="testResult"
        class="rounded-md border border-border bg-background/70 p-2 text-[11px] space-y-1"
      >
        <div class="flex items-center justify-between font-medium">
          <span>Test result ({{ testResult.status }})</span>
          <span v-if="testResult.plan" class="text-primary">{{ testResult.plan }}</span>
        </div>
        <div v-for="w in testResult.windows" :key="w.label" class="flex justify-between font-mono text-[10.5px]">
          <span class="text-muted-foreground">{{ w.label }}</span>
          <span>
            {{ Math.max(0, Math.round(100 - w.usedPercent)) }}% left
            <template v-if="w.detail"> ({{ w.detail }})</template>
          </span>
        </div>
        <p v-if="testResult.error" class="text-muted-foreground">{{ testResult.error }}</p>
      </div>

      <div class="flex flex-wrap items-center justify-between gap-2 pt-1">
        <div class="flex items-center gap-1.5">
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="h-7 text-xs"
            :disabled="testing || saving"
            @click="handleTest"
          >
            <LoaderCircleIcon v-if="testing" class="mr-1 size-3 animate-spin" />
            Test endpoint
          </Button>
          <Button
            v-if="hasStoredConfig"
            type="button"
            variant="ghost"
            size="sm"
            class="h-7 px-2 text-xs text-destructive hover:text-destructive"
            :disabled="saving"
            @click="handleDelete"
          >
            <Trash2Icon class="size-3.5" />
          </Button>
        </div>

        <div class="flex items-center gap-2">
          <span v-if="saved" class="text-[11px] text-primary">Saved</span>
          <Button
            type="button"
            size="sm"
            class="h-7 text-xs"
            :disabled="saving || testing"
            @click="handleSave"
          >
            Save usage config
          </Button>
        </div>
      </div>
    </div>
  </section>
</template>
