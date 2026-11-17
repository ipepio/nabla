<script setup lang="ts">
// One node of a tab's split tree: a leaf renders a pane, a split renders its two
// children in a row (side by side) or a column (stacked), separated by the app's
// own border token so the split is visible without drawing a handle.
//
// RECURSION — the component renders itself for the children. The depth handed down
// is "splits above this node", so a leaf knows whether it can still be split
// (`MAX_DEPTH` in `dock.ts`) and `DockPane.vue` is the single place that decides it.
// Nothing here enforces the cap: the model does, and hiding the button is a
// courtesy on top of it. The depth restarts at 0 for every tab, because the cap is
// per tab and never global.
//
// One root element for both shapes, so a leaf and a split occupy exactly the same
// flex cell: the only differences are the direction and the separators.
import { computed } from "vue";
import type { WorkspaceInfo } from "@/contracts/types";
import type { Pane } from "./dock";
import DockPane from "./DockPane.vue";

defineOptions({ name: "DockNode" });

const props = defineProps<{
  node: Pane;
  /** The tab this tree belongs to, so a pane can address its own layout. */
  tabId: string;
  /** Splits above this node; 0 is the tab's root pane. */
  depth: number;
  /** Whether this tab is the drawn one (a browser pane needs it, see DockPane). */
  active: boolean;
  workspace: WorkspaceInfo | null;
}>();

/** `row` lays the children side by side; everything else stacks them. */
const directionClass = computed(() => {
  if (props.node.kind === "leaf") return "flex-col";
  return props.node.direction === "row" ? "flex-row" : "flex-col";
});

/** The seam between the two children follows the split direction. */
const separatorClass = computed(() =>
  props.node.kind === "split" && props.node.direction === "row"
    ? "border-s"
    : "border-t",
);
</script>

<template>
  <div class="flex min-h-0 min-w-0 flex-1" :class="directionClass">
    <DockPane
      v-if="node.kind === 'leaf'"
      :tab-id="tabId"
      :instance="node.instance"
      :depth="depth"
      :active="active"
      :workspace="workspace"
    />

    <template v-else>
      <div class="flex min-h-0 min-w-0 flex-1">
        <DockNode
          :node="node.children[0]"
          :tab-id="tabId"
          :depth="depth + 1"
          :active="active"
          :workspace="workspace"
        />
      </div>
      <div class="flex min-h-0 min-w-0 flex-1" :class="separatorClass">
        <DockNode
          :node="node.children[1]"
          :tab-id="tabId"
          :depth="depth + 1"
          :active="active"
          :workspace="workspace"
        />
      </div>
    </template>
  </div>
</template>
