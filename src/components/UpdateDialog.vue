<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { ArrowDownToLineIcon, ExternalLinkIcon, LoaderCircleIcon } from "@lucide/vue";
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
import {
  commandErrorMessage,
  installUpdate,
  openExternalUrl,
  subscribeUpdaterProgress,
} from "@/contracts/commands";
import type { UpdateInfo, UpdateProgressEvent } from "@/contracts/types";

const open = defineModel<boolean>("open", { default: false });

const props = defineProps<{
  info: UpdateInfo | null;
}>();

const installing = ref(false);
const error = ref<string | null>(null);
const progress = ref<UpdateProgressEvent | null>(null);
let unlistenProgress: (() => void) | null = null;

const progressPercent = computed<number | null>(() => {
  const p = progress.value;
  if (!p || !p.total || p.total <= 0) return null;
  return Math.min(100, Math.round((p.downloaded / p.total) * 100));
});

const stageLabel = computed(() => {
  const p = progress.value;
  if (!p) return "Preparing update…";
  switch (p.stage) {
    case "downloading": {
      const mb = (p.downloaded / (1024 * 1024)).toFixed(1);
      const totalMb = p.total ? ` / ${(p.total / (1024 * 1024)).toFixed(1)} MB` : "";
      return `Downloading… ${mb} MB${totalMb}`;
    }
    case "verifying":
      return "Verifying SHA-256 signature…";
    case "installing":
      return "Installing update…";
    case "relaunching":
      return "Relaunching Nabla…";
    default:
      return p.message ?? "Updating…";
  }
});

async function handleInstall(): Promise<void> {
  if (!props.info || installing.value) return;
  installing.value = true;
  error.value = null;
  progress.value = {
    stage: "downloading",
    downloaded: 0,
    total: props.info.assetSize,
    message: null,
  };
  try {
    await installUpdate(props.info);
  } catch (err) {
    error.value = commandErrorMessage(err);
    installing.value = false;
  }
}

function openReleaseNotes(): void {
  if (props.info?.releaseUrl) {
    void openExternalUrl(props.info.releaseUrl);
  }
}

onMounted(async () => {
  unlistenProgress = await subscribeUpdaterProgress((ev) => {
    progress.value = ev;
  });
});

onBeforeUnmount(() => {
  unlistenProgress?.();
  unlistenProgress = null;
});
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent v-if="info" class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle class="flex items-center gap-2">
          <ArrowDownToLineIcon class="size-4 text-primary" />
          <span>Update available — v{{ info.latestVersion }}</span>
        </DialogTitle>
        <DialogDescription>
          Current version: <span class="font-mono">v{{ info.currentVersion }}</span>
          → <span class="font-mono text-foreground">v{{ info.latestVersion }}</span>
        </DialogDescription>
      </DialogHeader>

      <div
        v-if="info.notes"
        class="max-h-52 overflow-y-auto rounded-lg border border-border bg-secondary/40 p-3 text-xs whitespace-pre-wrap text-muted-foreground"
      >
        {{ info.notes }}
      </div>

      <div v-if="installing" class="space-y-1.5">
        <div class="flex items-center justify-between text-xs text-muted-foreground">
          <span class="flex items-center gap-1.5">
            <LoaderCircleIcon class="size-3.5 animate-spin text-primary" />
            {{ stageLabel }}
          </span>
          <span v-if="progressPercent !== null" class="font-mono tabular-nums">
            {{ progressPercent }}%
          </span>
        </div>
        <div class="h-1.5 w-full overflow-hidden rounded-full bg-muted">
          <div
            class="h-full rounded-full bg-primary transition-all"
            :style="{ width: `${progressPercent ?? 100}%` }"
          />
        </div>
      </div>

      <Alert v-if="error" variant="destructive">
        <AlertDescription class="text-xs">{{ error }}</AlertDescription>
      </Alert>

      <DialogFooter class="flex items-center justify-between sm:justify-between">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          class="gap-1.5 text-xs text-muted-foreground"
          :disabled="installing"
          @click="openReleaseNotes"
        >
          <ExternalLinkIcon class="size-3.5" />
          GitHub
        </Button>

        <div class="flex items-center gap-2">
          <DialogClose as-child>
            <Button type="button" variant="outline" size="sm" :disabled="installing">
              Later
            </Button>
          </DialogClose>
          <Button
            type="button"
            size="sm"
            :disabled="installing || !info.assetUrl"
            @click="handleInstall"
          >
            {{ installing ? "Updating…" : "Update now" }}
          </Button>
        </div>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
