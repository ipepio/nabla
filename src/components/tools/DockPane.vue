<script setup lang="ts">
// One pane of the dock: the header that identifies it, and the tool inside.
//
// The header is the whole pane contract, in four parts:
//   - the tool icon and label, from the registry (never hardcoded here);
//   - the CONTEXT CHIP — the folder this pane is pointed at (Files: its root,
//     Terminal: its cwd). Clicking it opens the native folder dialog, so a second
//     Files pane can look at another directory; the chip is also the terminal's
//     "change folder" affordance, and it is the only place that writes a context,
//     so a pane can be re-pointed but never half-re-pointed;
//   - split right / split down, hidden at the depth cap, on a singleton tool and
//     when the dock is already full (the dock's own cap, `dock.ts`);
//   - close, hidden when this is the only pane: a drawer with nothing in it is a
//     dead end (the empty face is reached by disabling the tool in Settings).
//
// MOUNTING — the tool is keyed by instance id AND by its folder. That is the rule
// that makes the model work: a pane re-pointed at another folder remounts, so the
// tree re-reads and the shell reopens there instead of showing the previous
// folder's answer. Two panes never share state because their ids differ.
//
// The component receives its own props from `binding`, one shape per tool: Files
// takes `rootPath` and emits `browse`, Terminal takes `cwd`, and the two
// read-only views take the active workspace.
import { computed } from "vue";
import { Columns2Icon, FolderIcon, Rows2Icon, XIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { pickDirectory } from "@/contracts/commands";
import type { WorkspaceInfo } from "@/contracts/types";
import { t } from "@/lib/i18n";
import {
  closePane,
  contextPath,
  dockTree,
  focusPane,
  focusedId,
  instances,
  MAX_DEPTH,
  MAX_PANES,
  setContextPath,
  splitPane,
  type SplitDirection,
} from "./dock";
import { TOOLS, toolById, type ToolInstance } from "./registry";

const props = defineProps<{
  instance: ToolInstance;
  /** Splits above this pane; 0 is the root pane. */
  depth: number;
  workspace: WorkspaceInfo | null;
}>();

/**
 * The registry entry of this instance. `toolId` is a `ToolId`, so a lookup can
 * only fail if the tree holds an instance the registry dropped; the fallback keeps
 * the pane renderable instead of blank in that impossible case.
 */
const definition = computed(() => toolById(props.instance.toolId) ?? TOOLS[0]);

/** The folder this pane shows, or `null` when the tool has no folder of its own. */
const path = computed(() => contextPath(props.instance));

const paneCount = computed(() => instances(dockTree.value).length);

const focused = computed(() => focusedId.value === props.instance.id);

/** The cap, in the three places the header must respect it. */
const canSplit = computed(
  () =>
    definition.value.singleton !== true &&
    props.depth < MAX_DEPTH &&
    paneCount.value < MAX_PANES,
);

const canClose = computed(() => paneCount.value > 1);

/**
 * What a remount is triggered by: the instance, and the folder it points at.
 * Changing folders therefore re-reads a tree and reopens a shell — the same
 * answer "switch workspace" already gave before the dock existed.
 */
const mountKey = computed(
  () => `${props.instance.id}:${path.value ?? ""}`,
);

/** The props of the tool inside, one shape per tool (see the module note). */
const binding = computed(() => {
  const instance = props.instance;
  switch (instance.toolId) {
    case "files":
      return { rootPath: instance.context.rootPath, onBrowse: browse };
    case "terminal":
      return { cwd: instance.context.cwd };
    case "changes":
    case "browser":
      return { workspace: props.workspace };
  }
});

async function browse(): Promise<void> {
  try {
    const selected = await pickDirectory(t("tools.dock.chooseFolder"));
    // A dismissed dialog answers `null`; a rejected one is left as it is: there
    // is no toast layer here, and the pane still points where it did.
    if (selected !== null) setContextPath(props.instance.id, selected);
  } catch {
    // Same as a dismissal: the pane keeps its folder.
  }
}

function split(direction: SplitDirection): void {
  splitPane(props.instance.id, direction);
}
</script>

<template>
  <div
    class="flex min-h-0 flex-1 flex-col overflow-hidden"
    @pointerdown="focusPane(instance.id)"
    @focusin="focusPane(instance.id)"
  >
    <div
      class="flex shrink-0 items-center gap-1 border-b px-1.5 py-1"
      :class="focused ? 'bg-muted/60' : ''"
    >
      <component
        :is="definition.icon"
        class="size-3.5 shrink-0 text-muted-foreground"
        aria-hidden="true"
      />
      <span class="shrink-0 text-xs font-medium">
        {{ t(definition.labelKey) }}
      </span>

      <!-- Context chip: the folder this pane is pointed at, one click away from
           another one. -->
      <button
        v-if="path !== null"
        type="button"
        class="flex min-w-0 items-center gap-1 rounded-sm px-1 py-0.5 text-[11px] text-muted-foreground outline-hidden hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
        :title="t('tools.dock.changeFolder', { path })"
        @click="browse"
      >
        <FolderIcon class="size-3 shrink-0" aria-hidden="true" />
        <span class="min-w-0 truncate">{{ path }}</span>
      </button>

      <span class="ms-auto flex shrink-0 items-center gap-0.5">
        <Button
          v-if="canSplit"
          variant="ghost"
          size="icon-xs"
          :aria-label="t('tools.dock.splitRight')"
          :title="t('tools.dock.splitRight')"
          @click="split('row')"
        >
          <Columns2Icon />
        </Button>
        <Button
          v-if="canSplit"
          variant="ghost"
          size="icon-xs"
          :aria-label="t('tools.dock.splitDown')"
          :title="t('tools.dock.splitDown')"
          @click="split('col')"
        >
          <Rows2Icon />
        </Button>
        <Button
          v-if="canClose"
          variant="ghost"
          size="icon-xs"
          :aria-label="t('tools.dock.close')"
          :title="t('tools.dock.close')"
          @click="closePane(instance.id)"
        >
          <XIcon />
        </Button>
      </span>
    </div>

    <component :is="definition.component" :key="mountKey" v-bind="binding" />
  </div>
</template>
