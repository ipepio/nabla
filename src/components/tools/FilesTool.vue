<script setup lang="ts">
// Files pane (S3 of `odd/tasks/tool-drawer-and-mcp.md`): a lazy tree of ONE
// folder, and the preview of the selected file in the SAME pane.
//
// The folder is `rootPath`, not the active workspace: a pane is an instance
// pointed at a directory, and the drawer shows as many Files panes as the dock has
// room for, each with its own root ("¿y si quiero abrir otro path porque quiero
// abrir otro archivo?"). The pane starts on the workspace and the path bar below
// re-points it at any other directory through the native folder dialog; the dock
// owns that value, so `browse` is an event and never a local state.
//
// REMOUNT, NOT WATCH: re-pointing a pane changes its dock context, and `DockPane`
// keys this component by it. The tree, the open folders and the selection are
// therefore dropped and re-read for the new root, which is exactly what a
// different folder means.
//
// Laziness: one `fsListDir` call reads the root, and every expansion reads
// exactly that one folder. Nothing recurses, so opening a folder costs one
// request whatever its depth, and a directory the user never opens is never read.
//
// Ownership: this component owns the tree state (children per relative path,
// which folders are open, which failed) and the selected path. The preview itself
// is `FilePreview`, which owns its request and its blob URL.
//
// Layout: the tree fills the pane; selecting a file gives it 60% of the height
// (`flex-[3]` against the tree's `flex-[2]`) and the X in the preview header
// restores the tree to full height.
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import {
  ChevronRightIcon,
  FileIcon,
  FolderIcon,
  FolderOpenIcon,
  FolderSearchIcon,
  LinkIcon,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { commandErrorMessage, fsListDir } from "@/contracts/commands";
import type { FsEntry } from "@/contracts/types";
import { t } from "@/lib/i18n";
import FilePreview from "./FilePreview.vue";
import { formatBytes } from "./format";

const props = defineProps<{
  /**
   * The absolute folder this pane browses. Empty means "no workspace yet", the
   * same face this pane had before the dock existed.
   */
  rootPath: string;
}>();

const emit = defineEmits<{
  /** The path bar's Browse button: the dock opens the dialog and writes the root. */
  (e: "browse"): void;
}>();

/** The pane's root as `fs_list_dir` takes it: an empty relative path. */
const ROOT = "";

/** Directories first, then names: the order a file manager uses. */
function sortEntries(entries: FsEntry[]): FsEntry[] {
  return [...entries].sort((left, right) => {
    const leftDir = left.kind === "dir" ? 0 : 1;
    const rightDir = right.kind === "dir" ? 0 : 1;
    if (leftDir !== rightDir) return leftDir - rightDir;
    return left.name.localeCompare(right.name);
  });
}

/** Read one level of the tree: the root, or one folder the user opened. */
const childrenByPath = ref<Record<string, FsEntry[]>>({});
const expandedPaths = ref<Set<string>>(new Set());
const pendingPaths = ref<Set<string>>(new Set());
const errorsByPath = ref<Record<string, string>>({});
const selectedPath = ref<string | null>(null);

let disposed = false;

async function loadDir(relPath: string): Promise<void> {
  const root = props.rootPath;
  if (root === "") return;
  if (pendingPaths.value.has(relPath)) return;
  pendingPaths.value.add(relPath);
  delete errorsByPath.value[relPath];
  try {
    const entries = await fsListDir(root, relPath);
    if (disposed) return;
    childrenByPath.value[relPath] = sortEntries(entries);
  } catch (error) {
    if (disposed) return;
    errorsByPath.value[relPath] = commandErrorMessage(error);
  } finally {
    if (!disposed) pendingPaths.value.delete(relPath);
  }
}

/**
 * The flattened visible tree: one row per entry, plus the inline rows a pending
 * or failed folder needs. Flattening (instead of a recursive component) keeps the
 * indentation and the arrow-key-free focus order in one place.
 */
type Row =
  | { kind: "entry"; entry: FsEntry; depth: number }
  | { kind: "loading"; path: string; depth: number }
  | { kind: "error"; path: string; depth: number; message: string }
  | { kind: "empty"; path: string; depth: number };

function walk(relPath: string, depth: number, out: Row[]): void {
  for (const entry of childrenByPath.value[relPath] ?? []) {
    out.push({ kind: "entry", entry, depth });
    if (entry.kind !== "dir" || !expandedPaths.value.has(entry.path)) continue;

    const message = errorsByPath.value[entry.path];
    if (message !== undefined) {
      out.push({ kind: "error", path: entry.path, depth: depth + 1, message });
      continue;
    }
    const entries = childrenByPath.value[entry.path];
    if (entries === undefined) {
      // Requested but not answered yet: the one row that says so.
      if (pendingPaths.value.has(entry.path)) {
        out.push({ kind: "loading", path: entry.path, depth: depth + 1 });
      }
      continue;
    }
    if (entries.length === 0) {
      out.push({ kind: "empty", path: entry.path, depth: depth + 1 });
      continue;
    }
    walk(entry.path, depth + 1, out);
  }
}

const rows = computed<Row[]>(() => {
  if (props.rootPath === "") return [];
  const out: Row[] = [];
  const rootError = errorsByPath.value[ROOT];
  if (rootError !== undefined) {
    out.push({ kind: "error", path: ROOT, depth: 0, message: rootError });
    return out;
  }
  if (childrenByPath.value[ROOT] === undefined) {
    if (pendingPaths.value.has(ROOT)) out.push({ kind: "loading", path: ROOT, depth: 0 });
    return out;
  }
  walk(ROOT, 0, out);
  return out;
});

/** Whether the root itself is still being read, for the empty face. */
const rootPending = computed(
  () =>
    props.rootPath !== "" &&
    pendingPaths.value.has(ROOT) &&
    childrenByPath.value[ROOT] === undefined,
);

function rowKey(row: Row): string {
  return row.kind === "entry" ? `e:${row.entry.path}` : `${row.kind}:${row.path}`;
}

function rowPadding(depth: number): string {
  return `${8 + depth * 12}px`;
}

/**
 * One click, one intent: a folder opens (and reads itself once), a file becomes
 * the preview. A symlink is treated as a file so Rust, not the UI, decides
 * whether it may be read at all.
 */
function activate(entry: FsEntry): void {
  if (entry.kind === "dir") {
    if (expandedPaths.value.has(entry.path)) {
      expandedPaths.value.delete(entry.path);
      return;
    }
    expandedPaths.value.add(entry.path);
    if (
      childrenByPath.value[entry.path] === undefined &&
      errorsByPath.value[entry.path] === undefined
    ) {
      void loadDir(entry.path);
    }
    return;
  }
  selectedPath.value = entry.path;
}

onMounted(() => {
  void loadDir(ROOT);
});

onBeforeUnmount(() => {
  disposed = true;
});
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div
      v-if="rootPath === ''"
      class="flex flex-1 items-center justify-center p-4"
    >
      <p class="text-center text-xs text-muted-foreground">
        {{ t("tools.files.noWorkspace") }}
      </p>
    </div>

    <template v-else>
      <!--
        Path bar: which folder this pane is showing, and the one action that
        makes a second Files pane useful — point it somewhere else.
      -->
      <div
        class="flex shrink-0 items-center gap-1.5 border-b px-2 py-1"
        :title="t('tools.files.root', { path: rootPath })"
      >
        <FolderOpenIcon
          class="size-3.5 shrink-0 text-muted-foreground"
          aria-hidden="true"
        />
        <span class="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">
          {{ rootPath }}
        </span>
        <Button variant="ghost" size="xs" class="shrink-0" @click="emit('browse')">
          <FolderSearchIcon />
          {{ t("tools.files.browse") }}
        </Button>
      </div>
      <!-- Tree: the whole tab, or the top 40% while a preview is open. -->
      <div
        class="min-h-0 overflow-auto py-1"
        :class="selectedPath === null ? 'flex-1' : 'flex-[2]'"
      >
        <div
          v-for="row in rows"
          :key="rowKey(row)"
          class="min-w-0"
        >
          <button
            v-if="row.kind === 'entry'"
            type="button"
            class="flex w-full min-w-0 items-center gap-1.5 rounded-sm py-1 pe-2 text-left text-xs outline-hidden hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
            :class="
              row.entry.path === selectedPath
                ? 'bg-muted text-foreground'
                : 'text-sidebar-foreground'
            "
            :style="{ paddingLeft: rowPadding(row.depth) }"
            :aria-expanded="
              row.entry.kind === 'dir'
                ? expandedPaths.has(row.entry.path)
                : undefined
            "
            :title="row.entry.path"
            @click="activate(row.entry)"
          >
            <template v-if="row.entry.kind === 'dir'">
              <ChevronRightIcon
                class="size-3 shrink-0 text-muted-foreground transition-transform"
                :class="expandedPaths.has(row.entry.path) ? 'rotate-90' : ''"
                aria-hidden="true"
              />
              <component
                :is="expandedPaths.has(row.entry.path) ? FolderOpenIcon : FolderIcon"
                class="size-3.5 shrink-0 text-muted-foreground"
                aria-hidden="true"
              />
            </template>
            <LinkIcon
              v-else-if="row.entry.kind === 'symlink'"
              class="size-3.5 shrink-0 text-muted-foreground"
              aria-hidden="true"
            />
            <FileIcon
              v-else
              class="size-3.5 shrink-0 text-muted-foreground"
              aria-hidden="true"
            />
            <span class="min-w-0 flex-1 truncate">{{ row.entry.name }}</span>
            <span
              v-if="row.entry.kind !== 'dir'"
              class="shrink-0 text-muted-foreground"
            >{{ formatBytes(row.entry.sizeBytes) }}</span>
          </button>

          <p
            v-else-if="row.kind === 'loading'"
            class="py-1 pe-2 text-xs text-muted-foreground"
            :style="{ paddingLeft: rowPadding(row.depth) }"
          >
            {{ t("common.loading") }}
          </p>

          <p
            v-else-if="row.kind === 'empty'"
            class="py-1 pe-2 text-xs text-muted-foreground"
            :style="{ paddingLeft: rowPadding(row.depth) }"
          >
            {{ t("tools.files.emptyDir") }}
          </p>

          <!-- A folder that could not be read says why, in place, and offers one
               retry for that folder only. -->
          <div
            v-else
            class="flex flex-col items-start gap-1 py-1 pe-2"
            :style="{ paddingLeft: rowPadding(row.depth) }"
          >
            <p class="text-xs text-destructive">
              {{ t("tools.files.loadError", { message: row.message }) }}
            </p>
            <Button variant="outline" size="xs" @click="loadDir(row.path)">
              {{ t("tools.files.retry") }}
            </Button>
          </div>
        </div>

        <p
          v-if="rows.length === 0 && rootPending"
          class="p-3 text-xs text-muted-foreground"
        >
          {{ t("common.loading") }}
        </p>
        <p
          v-else-if="rows.length === 0"
          class="p-3 text-xs text-muted-foreground"
        >
          {{ t("tools.files.emptyDir") }}
        </p>
      </div>

      <!-- Preview, in the same tab, at ~60% of the height. -->
      <div v-if="selectedPath !== null" class="flex min-h-0 flex-[3] flex-col">
        <FilePreview
          :root-path="rootPath"
          :rel-path="selectedPath"
          @close="selectedPath = null"
        />
      </div>
    </template>
  </div>
</template>
