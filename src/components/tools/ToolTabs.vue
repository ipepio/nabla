<script lang="ts">
// Module state, on purpose: the drawer unmounts when it is collapsed, so a
// component-local `ref` would forget the tab. This module keeps the last active
// tool for the whole app session (S2: no persistence across launches — that is a
// settings key, out of scope here).
import { ref } from "vue";
import { DEFAULT_TOOL_ID, type ToolId } from "./registry";

const rememberedToolId = ref<ToolId>(DEFAULT_TOOL_ID);
</script>

<script setup lang="ts">
// The drawer's tab host (F1 of `odd/tasks/tool-drawer-and-mcp.md`).
//
// It owns exactly three things: which tool is active, the tab strip, and mounting
// the active tool. The tools themselves come from `registry.ts`, so a new tool is
// one entry there and never an edit to this file.
//
// The tool is remounted with `key` = (tool, workspace), which is what makes
// switching workspaces RE-READ the folder or the working tree instead of showing
// the previous workspace's data.
import { computed } from "vue";
import type { WorkspaceInfo } from "@/contracts/types";
import { t } from "@/lib/i18n";
import { TOOLS, type ToolDefinition } from "./registry";

defineProps<{ workspace: WorkspaceInfo | null }>();

const activeId = rememberedToolId;

/**
 * The active entry, or the first one when the remembered id is no longer in the
 * registry (a tool can disappear between releases).
 */
const activeTool = computed<ToolDefinition | null>(
  () => TOOLS.find((tool) => tool.id === activeId.value) ?? TOOLS[0] ?? null,
);

function selectTool(tool: ToolDefinition): void {
  activeId.value = tool.id;
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <!--
      Tab strip: one equal-width button per tool. Labels truncate on a narrow
      drawer instead of wrapping, so the strip keeps a single compact row.
    -->
    <div
      v-if="TOOLS.length > 0"
      role="tablist"
      :aria-label="t('tools.tabs')"
      class="flex shrink-0 items-center gap-1 border-b px-1.5 py-1.5"
    >
      <button
        v-for="tool in TOOLS"
        :key="tool.id"
        type="button"
        role="tab"
        :aria-selected="tool.id === activeTool?.id"
        class="flex min-w-0 flex-1 items-center justify-center gap-1.5 rounded-[min(var(--radius-md),8px)] px-2 py-1 text-xs outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring"
        :class="
          tool.id === activeTool?.id
            ? 'bg-muted text-foreground'
            : 'text-muted-foreground hover:bg-muted/60 hover:text-foreground'
        "
        @click="selectTool(tool)"
      >
        <component :is="tool.icon" class="size-3.5 shrink-0" aria-hidden="true" />
        <span class="truncate">{{ t(tool.labelKey) }}</span>
      </button>
    </div>

    <!--
      The active tool. `key` includes the workspace so a switch remounts it: the
      tree and the change list belong to one workspace and must never be reused
      for another.
    -->
    <div
      v-if="activeTool"
      role="tabpanel"
      :aria-label="t(activeTool.labelKey)"
      class="flex min-h-0 flex-1 flex-col"
    >
      <component
        :is="activeTool.component"
        :key="`${activeTool.id}:${workspace?.id ?? 'none'}`"
        :workspace="workspace"
      />
    </div>

    <div v-else class="flex flex-1 items-center justify-center p-4">
      <p class="text-center text-xs text-muted-foreground">
        {{ t("tools.empty") }}
      </p>
    </div>
  </div>
</template>
