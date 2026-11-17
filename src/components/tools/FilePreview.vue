<script setup lang="ts">
// The preview of one file, rendered inside the Files tab (S3 of
// `odd/tasks/tool-drawer-and-mcp.md`: "cuando lo selecciones, te lo abre en la
// misma pestaña").
//
// What it renders is decided by Rust, not guessed here: `fsReadPreview` answers
// `text` (with its content), `image` (bytes fetched separately), `binary` or
// `too_large`. This component therefore never reads a file itself and never
// streams bytes the backend refused to classify.
//
// Bounds, in order of authority:
//   1. Rust's own size cap (a `too_large` answer carries no content);
//   2. `MAX_TEXT_CHARS` below, so a huge-but-legal text file cannot build a
//      multi-megabyte DOM node; the truncation is stated, never silent;
//   3. the preview panel's own height (`flex-[3]` of the tab), which scrolls.
//
// Image bytes cross the seam as a raw binary body (`tauri::ipc::Response`), so the
// only thing built here is ONE object URL, revoked as soon as the preview changes
// or the component unmounts: no blob leaks per click, and no base64 round trip.
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { ImageIcon, XIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  commandErrorMessage,
  fsReadImageBytes,
  fsReadPreview,
} from "@/contracts/commands";
import type { FsPreview, WorkspaceInfo } from "@/contracts/types";
import { t } from "@/lib/i18n";
import { formatBytes } from "./format";

const props = defineProps<{
  workspace: WorkspaceInfo;
  /** Workspace-relative path of the file to preview. */
  relPath: string;
}>();

const emit = defineEmits<{
  /** The X: the Files tab goes back to a full-height tree. */
  (e: "close"): void;
}>();

/** The most text this pane will put in the DOM, whatever Rust allowed. */
const MAX_TEXT_CHARS = 200_000;

const preview = ref<FsPreview | null>(null);
const imageUrl = ref<string | null>(null);
const loadError = ref<string | null>(null);
const loading = ref(false);

/**
 * The answer of a request that has been superseded must never reach the DOM: the
 * user can click two files faster than a large file reads. Each load gets an id
 * and only the newest one writes state.
 */
let requestId = 0;
let disposed = false;

function releaseImage(): void {
  if (imageUrl.value === null) return;
  URL.revokeObjectURL(imageUrl.value);
  imageUrl.value = null;
}

async function load(): Promise<void> {
  const id = (requestId += 1);
  loading.value = true;
  loadError.value = null;
  preview.value = null;
  releaseImage();
  try {
    const answer = await fsReadPreview(props.workspace.path, props.relPath);
    if (disposed || id !== requestId) return;
    preview.value = answer;
    if (answer.kind === "image") {
      const bytes = await fsReadImageBytes(props.workspace.path, props.relPath);
      if (disposed || id !== requestId) return;
      // The mime comes from the preview; a name whose type Rust did not model
      // still renders, as an opaque binary body the <img> can sniff.
      const mime = answer.mime ?? "application/octet-stream";
      imageUrl.value = URL.createObjectURL(new Blob([bytes], { type: mime }));
    }
  } catch (error) {
    if (disposed || id !== requestId) return;
    loadError.value = commandErrorMessage(error);
  } finally {
    if (!disposed && id === requestId) loading.value = false;
  }
}

/** The text to render, and whether it was cut at `MAX_TEXT_CHARS`. */
const textBody = computed(() => {
  const content =
    preview.value?.kind === "text" ? (preview.value.content ?? "") : "";
  return {
    content: content.slice(0, MAX_TEXT_CHARS),
    truncated: content.length > MAX_TEXT_CHARS,
  };
});

watch(() => [props.workspace.path, props.relPath], load);
onMounted(load);
onBeforeUnmount(() => {
  disposed = true;
  releaseImage();
});
</script>

<template>
  <section class="flex min-h-0 flex-1 flex-col border-t" aria-live="polite">
    <!-- Header: the workspace-relative path is the identity of what is shown,
         plus the size and the close control. -->
    <div class="flex shrink-0 items-center gap-2 px-2 py-1.5">
      <span class="min-w-0 flex-1 truncate text-xs font-medium" :title="relPath">
        {{ relPath }}
      </span>
      <span
        v-if="preview"
        class="shrink-0 text-xs text-muted-foreground"
      >{{ formatBytes(preview.sizeBytes) }}</span>
      <Button
        variant="ghost"
        size="icon-xs"
        class="shrink-0"
        :aria-label="t('tools.preview.close')"
        @click="emit('close')"
      >
        <XIcon />
      </Button>
    </div>

    <div class="min-h-0 flex-1 overflow-auto">
      <p v-if="loading" class="p-3 text-xs text-muted-foreground">
        {{ t("common.loading") }}
      </p>

      <!-- Errors stay in the pane: the tree keeps its place behind them. -->
      <div v-else-if="loadError" class="flex flex-col items-start gap-2 p-3">
        <p class="text-xs text-destructive">{{ loadError }}</p>
        <Button variant="outline" size="xs" @click="load">
          {{ t("tools.files.retry") }}
        </Button>
      </div>

      <template v-else-if="preview">
        <!-- Text: monospace, original whitespace and line breaks preserved. -->
        <template v-if="preview.kind === 'text'">
          <p v-if="textBody.content === ''" class="p-3 text-xs text-muted-foreground">
            {{ t("tools.preview.empty") }}
          </p>
          <template v-else>
            <p
              v-if="textBody.truncated"
              class="border-b px-3 py-1.5 text-xs text-warning"
            >
              {{ t("tools.preview.truncated", { count: MAX_TEXT_CHARS }) }}
            </p>
            <pre
              class="m-0 whitespace-pre p-3 font-mono text-xs leading-5"
            >{{ textBody.content }}</pre>
          </template>
        </template>

        <!-- Image: the object URL built from the raw bytes the backend sent. -->
        <div
          v-else-if="preview.kind === 'image'"
          class="flex h-full items-start justify-center p-3"
        >
          <img
            v-if="imageUrl"
            :src="imageUrl"
            :alt="t('tools.preview.imageAlt', { path: relPath })"
            class="max-h-full max-w-full object-contain"
          />
          <span v-else class="flex items-center gap-2 text-xs text-muted-foreground">
            <ImageIcon class="size-4" aria-hidden="true" />
            {{ t("common.loading") }}
          </span>
        </div>

        <p
          v-else-if="preview.kind === 'too_large'"
          class="p-3 text-xs text-muted-foreground"
        >
          {{ t("tools.preview.tooLarge", { size: formatBytes(preview.sizeBytes) }) }}
        </p>

        <p v-else class="p-3 text-xs text-muted-foreground">
          {{ t("tools.preview.binary") }}
        </p>
      </template>
    </div>
  </section>
</template>
