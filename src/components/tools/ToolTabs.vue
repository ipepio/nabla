<script lang="ts">
// The dock's layout lives at MODULE scope, not in this component.
//
// Why: the drawer unmounts whenever it is collapsed (the parent keeps only
// `toolsOpen`), so a component-local `ref` would throw the panes away every time
// the human hides the panel. The tree itself — and the rules around it — live in
// `./dock.ts`; this file is only the host that renders it and offers the way to
// add a tool.
</script>

<script setup lang="ts">
// The drawer's tool host: a DOCK of tool instances with splits, not a tab strip.
//
// The request this replaces a tab strip for is literal: "un selector de tools y
// que tú puedas abrir las que quieras en el drawer… ¿y si quiero abrir otro path
// porque quiero abrir otro archivo? ¿y si quiero dividir la pestaña del drawer
// porque quiero 2 o n files? Lo mismo con la consola."
//
// So this component owns exactly three things:
//
//  1. the "+ Add tool" menu — the selector, built from `visibleTools`, i.e. the
//     tools the human enabled in Settings (S5), never a hardcoded list;
//  2. reconciling the module-scope dock with reality (`applyDock`): a new active
//     workspace resets the layout, a disabled tool closes its panes;
//  3. the empty face, which is the same "no tools" state the tab strip had, plus
//     the "nothing open yet" one the dock can reach.
//
// Adding a tool is one registry entry (`./registry.ts`), never an edit here. Each
// pane's header, splits and folder chip belong to `DockPane.vue`; the layout tree
// is `DockNode.vue`.
import { computed, watch } from "vue";
import { PlusIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { WorkspaceInfo } from "@/contracts/types";
import { t } from "@/lib/i18n";
import { enabledTools } from "@/lib/preferences";
import DockNode from "./DockNode.vue";
import {
  addPreview,
  addTool,
  applyDock,
  dockTree,
  instances,
  MAX_PANES,
} from "./dock";
import { visibleTools, type ToolDefinition } from "./registry";

const props = defineProps<{ workspace: WorkspaceInfo | null }>();

/** S5: the enabled tools, in registry order — the whole "+ Add tool" menu. */
const available = computed<ToolDefinition[]>(() =>
  visibleTools(enabledTools.value),
);

const panes = computed(() => instances(dockTree.value));

/** At least one entry can still be opened (or focused), so the menu is worth a click. */
const canAddAny = computed(() =>
  available.value.some((tool) => addPreview(tool.id) !== "full"),
);

/**
 * The dock is reconciled here because this is the only place that knows both
 * inputs: the active workspace and the enabled tools. `immediate` covers the
 * drawer being reopened after the human flipped a switch while it was collapsed.
 */
watch(
  [() => props.workspace?.path ?? null, () => enabledTools.value.join(",")],
  () => applyDock(props.workspace, enabledTools.value),
  { immediate: true },
);

/** What a click on one menu entry would do, without doing it (`dock.ts`). */
function preview(toolId: ToolDefinition["id"]) {
  return addPreview(toolId);
}

function open(toolId: ToolDefinition["id"]): void {
  addTool(toolId, props.workspace);
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <!--
      Dock bar: the selector. Compact on purpose — the drawer is 240px wide at its
      narrowest, and the panes need the height. The counter states the cap so
      "Add tool" being unavailable is explained instead of surprising.
    -->
    <div
      v-if="available.length > 0"
      class="flex shrink-0 items-center gap-1.5 border-b px-1.5 py-1.5"
    >
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button
            variant="ghost"
            size="xs"
            :disabled="!canAddAny"
            :title="canAddAny ? t('tools.dock.add') : t('tools.dock.full', { count: MAX_PANES })"
          >
            <PlusIcon />
            {{ t("tools.dock.add") }}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" class="w-52">
          <DropdownMenuLabel v-if="!canAddAny">
            {{ t("tools.dock.full", { count: MAX_PANES }) }}
          </DropdownMenuLabel>
          <DropdownMenuItem
            v-for="tool in available"
            :key="tool.id"
            :disabled="preview(tool.id) === 'full'"
            @select="open(tool.id)"
          >
            <component :is="tool.icon" />
            {{ t(tool.labelKey) }}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <span class="ms-auto shrink-0 text-[11px] text-muted-foreground">
        {{
          t("tools.dock.panes", {
            count: panes.length,
            max: MAX_PANES,
          })
        }}
      </span>
    </div>

    <!-- The layout: a split tree whose leaves are the open panes. -->
    <DockNode
      v-if="dockTree"
      :node="dockTree"
      :depth="0"
      :workspace="workspace"
    />

    <!-- Nothing open: either nothing to open, or an empty dock. -->
    <div v-else class="flex flex-1 items-center justify-center p-4">
      <p class="text-center text-xs text-muted-foreground">
        {{ available.length === 0 ? t("tools.empty") : t("tools.dock.empty") }}
      </p>
    </div>
  </div>
</template>
