<script setup lang="ts">
// Changes tab (S2, S3 of `odd/tasks/tool-drawer-and-mcp.md`): the working tree of
// the active workspace, and the diff of one file in the SAME tab.
//
// Read-only by design: this tab lists what `git status` reports and shows the
// diff `git diff` prints. It has no staging and no commit control — those are
// terminal/agent work, and adding them here would put a write path in a view.
//
// Grouping: one path can appear twice, once in the index and once in the working
// tree (staged, then edited again). `git_status` reports those as two entries, so
// the list is grouped exactly that way — Staged, Unstaged, Untracked — and each
// row is keyed by group + path so the two never collide.
//
// Freshness: manual Refresh, plus one refresh when the window regains focus,
// which is when the user comes back from the editor or the terminal.
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { RefreshCwIcon, XIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  commandErrorMessage,
  fsListDir,
  gitDiff,
  gitStatus,
} from "@/contracts/commands";
import type { GitEntry, GitEntryStatus, WorkspaceInfo } from "@/contracts/types";
import { t, type MessageKey } from "@/lib/i18n";

const props = defineProps<{
  workspace: WorkspaceInfo | null;
}>();

/** The most diff text this pane puts in the DOM; the rest is announced, not cut. */
const MAX_DIFF_CHARS = 200_000;

/**
 * The letter and the semantic colour of each change. Colours are Tailwind token
 * classes only (`success`, `warning`, `destructive`, `primary`), never hex: the
 * palette lives in `.env` and `theme.ts` (AGENTS.md 9).
 */
const STATUS_META: Record<
  GitEntryStatus,
  { letter: string; class: string; labelKey: MessageKey }
> = {
  modified: {
    letter: "M",
    class: "text-warning",
    labelKey: "tools.changes.status.modified",
  },
  added: {
    letter: "A",
    class: "text-success",
    labelKey: "tools.changes.status.added",
  },
  deleted: {
    letter: "D",
    class: "text-destructive",
    labelKey: "tools.changes.status.deleted",
  },
  renamed: {
    letter: "R",
    class: "text-primary",
    labelKey: "tools.changes.status.renamed",
  },
  untracked: {
    letter: "?",
    class: "text-muted-foreground",
    labelKey: "tools.changes.status.untracked",
  },
  conflicted: {
    letter: "U",
    class: "text-destructive",
    labelKey: "tools.changes.status.conflicted",
  },
};

const entries = ref<GitEntry[]>([]);
const loadError = ref<string | null>(null);
const loading = ref(false);
/**
 * Whether the workspace ROOT is a git checkout, asked only when the list is
 * empty. `git_status` answers an empty list for a folder that is not a
 * repository (`src-tauri/src/workspace.rs`), so without this probe the two empty
 * faces — clean tree, no repository — would be one ambiguous answer.
 *
 * The test is the `.git` entry of the root: exact when the workspace is the
 * repository root, which is the case this tab is for. A workspace that is a
 * clean SUBDIRECTORY of a repository has no `.git` of its own and then reads
 * "not a git repository"; the ambiguity is the backend contract's, and it is
 * reported rather than hidden. `null` means the probe could not run, and the
 * pane then says the quieter thing (clean tree).
 */
async function probeRepoRoot(workspacePath: string): Promise<boolean | null> {
  try {
    const entries = await fsListDir(workspacePath, "");
    return entries.some((entry) => entry.name === ".git");
  } catch {
    return null;
  }
}

const rootIsRepo = ref<boolean | null>(null);

const selectedPath = ref<string | null>(null);
const diffText = ref<string | null>(null);
const diffError = ref<string | null>(null);
const diffLoading = ref(false);

let disposed = false;
let statusRequestId = 0;
let diffRequestId = 0;

/**
 * One refresh at a time: a focus refresh landing on top of a manual one would
 * only race the same answer, and the newest request is the one that counts.
 */
async function load(): Promise<void> {
  const workspace = props.workspace;
  if (workspace === null) return;
  const id = (statusRequestId += 1);
  loading.value = true;
  try {
    const answer = await gitStatus(workspace.path);
    if (disposed || id !== statusRequestId) return;
    // Only an empty list needs the extra question; a non-empty one proves the
    // folder is a repository by itself.
    const isRepo =
      answer.length > 0 ? true : await probeRepoRoot(workspace.path);
    if (disposed || id !== statusRequestId) return;
    entries.value = answer;
    rootIsRepo.value = isRepo;
    loadError.value = null;
    // A selection the working tree no longer reports would open a diff of
    // nothing; drop it instead of leaving a pane that cannot be explained.
    if (
      selectedPath.value !== null &&
      !answer.some((entry) => entry.path === selectedPath.value)
    ) {
      clearDiff();
    }
  } catch (error) {
    if (disposed || id !== statusRequestId) return;
    entries.value = [];
    rootIsRepo.value = null;
    loadError.value = commandErrorMessage(error);
  } finally {
    if (!disposed && id === statusRequestId) loading.value = false;
  }
}

interface Group {
  id: "staged" | "unstaged" | "untracked";
  labelKey: MessageKey;
  rows: GitEntry[];
}

/** The three groups, empty ones omitted: no heading over nothing. */
const groups = computed<Group[]>(() => {
  const all = entries.value;
  return (
    [
      {
        id: "staged",
        labelKey: "tools.changes.staged",
        rows: all.filter((entry) => entry.staged),
      },
      {
        id: "unstaged",
        labelKey: "tools.changes.unstaged",
        rows: all.filter(
          (entry) => !entry.staged && entry.status !== "untracked",
        ),
      },
      {
        id: "untracked",
        labelKey: "tools.changes.untracked",
        rows: all.filter(
          (entry) => !entry.staged && entry.status === "untracked",
        ),
      },
    ] satisfies Group[]
  ).filter((group) => group.rows.length > 0);
});

/** The text to render, and whether it was cut at `MAX_DIFF_CHARS`. */
const diffBody = computed(() => {
  const text = diffText.value ?? "";
  return {
    content: text.slice(0, MAX_DIFF_CHARS),
    truncated: text.length > MAX_DIFF_CHARS,
  };
});

function clearDiff(): void {
  selectedPath.value = null;
  diffText.value = null;
  diffError.value = null;
  diffLoading.value = false;
  diffRequestId += 1;
}

/** Read one file's diff, superseding whatever was being read before it. */
async function loadDiff(path: string): Promise<void> {
  const workspace = props.workspace;
  if (workspace === null) return;
  selectedPath.value = path;
  diffText.value = null;
  diffError.value = null;
  const id = (diffRequestId += 1);
  diffLoading.value = true;
  try {
    const answer = await gitDiff(workspace.path, path);
    if (disposed || id !== diffRequestId) return;
    diffText.value = answer;
  } catch (error) {
    if (disposed || id !== diffRequestId) return;
    diffError.value = commandErrorMessage(error);
  } finally {
    if (!disposed && id === diffRequestId) diffLoading.value = false;
  }
}

/** Lazy per file: the list never diffs anything the user did not click. */
function openDiff(entry: GitEntry): void {
  void loadDiff(entry.path);
}

// Coming back from the editor or the terminal is exactly when the tree changed.
function onWindowFocus(): void {
  void load();
}

onMounted(() => {
  void load();
  window.addEventListener("focus", onWindowFocus);
});

onBeforeUnmount(() => {
  disposed = true;
  window.removeEventListener("focus", onWindowFocus);
});
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div class="flex shrink-0 items-center px-2 py-1">
      <Button
        variant="ghost"
        size="xs"
        class="ms-auto"
        :disabled="loading || workspace === null"
        @click="load"
      >
        <RefreshCwIcon :class="loading ? 'animate-spin' : ''" aria-hidden="true" />
        {{ loading ? t("tools.changes.refreshing") : t("tools.changes.refresh") }}
      </Button>
    </div>

    <div
      v-if="workspace === null"
      class="flex flex-1 items-center justify-center p-4"
    >
      <p class="text-center text-xs text-muted-foreground">
        {{ t("tools.changes.noWorkspace") }}
      </p>
    </div>

    <template v-else>
      <div
        class="min-h-0 overflow-auto pb-1"
        :class="selectedPath === null ? 'flex-1' : 'flex-[2]'"
      >
        <div v-if="loading && entries.length === 0" class="p-3">
          <p class="text-xs text-muted-foreground">{{ t("common.loading") }}</p>
        </div>

        <!-- Two empty faces: a clean checkout and a folder that is not a
             repository look identical from `git_status`, so the root probe
             decides which one to say. Any other failure keeps git's reason. -->
        <div v-else-if="loadError" class="flex flex-col items-start gap-2 p-3">
          <p class="text-xs text-destructive">
            {{ t("tools.changes.error", { message: loadError }) }}
          </p>
          <Button variant="outline" size="xs" @click="load">
            {{ t("tools.changes.refresh") }}
          </Button>
        </div>

        <p
          v-else-if="groups.length === 0"
          class="p-3 text-xs text-muted-foreground"
        >
          {{
            rootIsRepo === false
              ? t("tools.changes.emptyNoRepo")
              : t("tools.changes.emptyClean")
          }}
        </p>

        <div v-for="group in groups" :key="group.id">
          <p
            class="px-2 pt-2 pb-1 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase"
          >
            {{ t(group.labelKey) }}
          </p>
          <button
            v-for="entry in group.rows"
            :key="`${group.id}:${entry.path}`"
            type="button"
            class="flex w-full min-w-0 items-center gap-2 rounded-sm px-2 py-1 text-left text-xs outline-hidden hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
            :class="entry.path === selectedPath ? 'bg-muted text-foreground' : ''"
            :title="`${t(STATUS_META[entry.status].labelKey)} · ${entry.path}`"
            @click="openDiff(entry)"
          >
            <span
              class="w-3 shrink-0 font-mono font-semibold"
              :class="STATUS_META[entry.status].class"
              aria-hidden="true"
            >
              {{ STATUS_META[entry.status].letter }}
            </span>
            <span
              class="min-w-0 flex-1 truncate"
              :title="entry.path"
            >{{ entry.path }}</span>
            <span class="sr-only">
              {{ t(STATUS_META[entry.status].labelKey) }}
            </span>
          </button>
        </div>
      </div>

      <!-- Diff of the selected file, in this same tab. -->
      <div
        v-if="selectedPath !== null"
        class="flex min-h-0 flex-[3] flex-col border-t"
      >
        <div class="flex shrink-0 items-center gap-2 px-2 py-1.5">
          <span
            class="min-w-0 flex-1 truncate text-xs font-medium"
            :title="selectedPath"
          >{{ selectedPath }}</span>
          <Button
            variant="ghost"
            size="icon-xs"
            class="shrink-0"
            :aria-label="t('tools.changes.closeDiff')"
            @click="clearDiff"
          >
            <XIcon />
          </Button>
        </div>

        <div class="min-h-0 flex-1 overflow-auto">
          <p v-if="diffLoading" class="p-3 text-xs text-muted-foreground">
            {{ t("common.loading") }}
          </p>

          <div v-else-if="diffError" class="flex flex-col items-start gap-2 p-3">
            <p class="text-xs text-destructive">
              {{ t("tools.changes.diffError", { message: diffError }) }}
            </p>
            <Button
              variant="outline"
              size="xs"
              @click="loadDiff(selectedPath)"
            >
              {{ t("tools.changes.refresh") }}
            </Button>
          </div>

          <p
            v-else-if="diffBody.content === ''"
            class="p-3 text-xs text-muted-foreground"
          >
            {{ t("tools.changes.diffEmpty") }}
          </p>

          <template v-else>
            <p
              v-if="diffBody.truncated"
              class="border-b px-3 py-1.5 text-xs text-warning"
            >
              {{ t("tools.preview.truncated", { count: MAX_DIFF_CHARS }) }}
            </p>
            <pre
              class="m-0 whitespace-pre p-2 font-mono text-xs leading-5"
            >{{ diffBody.content }}</pre>
          </template>
        </div>
      </div>
    </template>
  </div>
</template>
