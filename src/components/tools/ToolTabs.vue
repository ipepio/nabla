<script lang="ts">
// The drawer's tab model lives at MODULE scope, not in this component.
//
// Why: the host that renders the tabs can come and go (it is mounted with the
// drawer, and the drawer's own visibility is its parent's business), so a
// component-local `ref` would throw the tabs away with it. The tabs themselves —
// and the rules around them — live in `./dock.ts`; this file is only the host that
// renders them.
</script>

<script setup lang="ts">
// The drawer's tool host: TABS of split panes.
//
// The request is literal: "No quiero tener un máximo de tools, mi idea es que el
// drawer esté vacío y pueda seleccionar las tools que quiera, en el drawer que
// pueda tener pestañas, estas pestañas es un área vacía y puedo arrancar la tool
// que esté disponible. Y ya después, poder dividir una pestaña hasta un máximo de
// 4."
//
// So this component owns exactly four things:
//
//  1. the TAB STRIP — scrollable, one entry per tab, `+` at the end, `×` per tab,
//     and the active tab drawn with the sidebar tokens the app already uses for
//     its own navigation. There is no limit on tabs;
//  2. the EMPTY TAB body — the tool chooser. A new tab is an empty area, and
//     starting a tool there turns it into the tab's single pane. The chooser is
//     built from `visibleTools`, i.e. the tools the human enabled in Settings
//     (S5), never a hardcoded list;
//  3. reconciling the model with reality (`applyDock`): a new active workspace
//     clears the tabs, a disabled tool closes its panes;
//  4. the "no tabs" empty state, which is what the drawer shows before the first
//     `+` — nothing is opened for the human, by design.
//
// EVERY TAB BODY IS RENDERED, AND ONLY ONE IS SHOWN (`v-show`). That is the rule
// that keeps tool state across switches for free: a Terminal's pty lives in its
// component, so hiding the body keeps the shell and its scrollback instead of
// reopening it on every tab switch. The one tool a CSS hide cannot serve is the
// Browser — its view is a child webview, which floats above the HTML — so the pane
// passes `active` and `visible` down and `BrowserTool.vue` closes/rebuilds its view
// (a tab that stopped being drawn) or hides/shows it (the drawer did), which is the
// difference between restarting a page and leaving it alone.
//
// Adding a tool is one registry entry (`./registry.ts`), never an edit here. Each
// pane's header, splits and folder chip belong to `DockPane.vue`; a tab's layout
// tree is `DockNode.vue`.
import { computed, watch, type Component } from "vue";
import { PlusIcon, XIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import type { WorkspaceInfo } from "@/contracts/types";
import { t } from "@/lib/i18n";
import { enabledTools } from "@/lib/preferences";
import DockNode from "./DockNode.vue";
import {
  activateTab,
  activeTabId,
  addTab,
  applyDock,
  closeTab,
  setTabTool,
  tabPanes,
  tabTitleKey,
  tabs,
  type DockTab,
} from "./dock";
import { toolById, visibleTools, type ToolDefinition } from "./registry";

const props = withDefaults(
  defineProps<{
    workspace: WorkspaceInfo | null;
    /**
     * The drawer around this host is on screen. Only one tool cares — a child
     * webview is not hidden by CSS — and absent means "on screen", so a host
     * rendered somewhere else keeps working.
     */
    visible?: boolean;
  }>(),
  { visible: true },
);

/** S5: the enabled tools, in registry order — the whole tool chooser. */
const available = computed<ToolDefinition[]>(() =>
  visibleTools(enabledTools.value),
);

/**
 * The tabs are reconciled here because this is the only place that knows both
 * inputs: the active workspace and the enabled tools. `immediate` covers the drawer
 * being reopened after the human flipped a switch while it was collapsed.
 */
watch(
  [() => props.workspace?.path ?? null, () => enabledTools.value.join(",")],
  () => applyDock(props.workspace, enabledTools.value),
  { immediate: true },
);

/** The tool icon of a tab's first pane, or `null` for an empty tab. */
function tabIcon(tab: DockTab): Component | null {
  const first = tabPanes(tab)[0];
  if (first === undefined) return null;
  return toolById(first.toolId)?.icon ?? null;
}

/** A tab is named after the tool of its first pane, and "New tab" while empty. */
function tabLabel(tab: DockTab): string {
  return tab.title ?? t(tabTitleKey(tab));
}

/** Starts one enabled tool in an empty tab (the model refuses a busy one). */
function start(tabId: string, toolId: ToolDefinition["id"]): void {
  setTabTool(tabId, toolId, props.workspace);
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <!--
      Tab strip: the sidebar's own tokens, so it reads as navigation and not as a
      second toolbar. It scrolls horizontally instead of wrapping (the drawer is
      240px wide at its narrowest) and the `+` stays outside the scroll area, so it
      is reachable with any number of tabs.
    -->
    <div
      class="flex shrink-0 items-stretch border-b border-sidebar-border bg-sidebar text-sidebar-foreground"
    >
      <div class="flex min-w-0 flex-1 items-stretch overflow-x-auto">
        <div
          v-for="tab in tabs"
          :key="tab.id"
          class="group/tab flex min-w-0 shrink-0 items-center border-e border-sidebar-border"
          :class="
            tab.id === activeTabId
              ? 'bg-sidebar-accent text-sidebar-accent-foreground'
              : 'text-sidebar-foreground/70 hover:bg-sidebar-accent/50 hover:text-sidebar-accent-foreground'
          "
        >
          <button
            type="button"
            class="flex min-w-0 max-w-40 items-center gap-1 truncate px-1.5 py-1.5 text-xs outline-hidden focus-visible:ring-2 focus-visible:ring-sidebar-ring focus-visible:ring-inset"
            :class="tab.id === activeTabId ? 'font-medium' : ''"
            :title="tabLabel(tab)"
            @click="activateTab(tab.id)"
          >
            <component
              :is="tabIcon(tab)"
              v-if="tabIcon(tab) !== null"
              class="size-3.5 shrink-0 opacity-70"
              aria-hidden="true"
            />
            <span class="truncate">{{ tabLabel(tab) }}</span>
          </button>
          <Button
            variant="ghost"
            size="icon-xs"
            class="me-0.5 size-5 shrink-0 opacity-60 group-hover/tab:opacity-100"
            :aria-label="t('tools.tabs.close')"
            :title="t('tools.tabs.close')"
            @click.stop="closeTab(tab.id)"
          >
            <XIcon />
          </Button>
        </div>
      </div>

      <Button
        variant="ghost"
        size="icon-xs"
        class="m-0.5 size-5 shrink-0 self-center"
        :aria-label="t('tools.tabs.new')"
        :title="t('tools.tabs.new')"
        @click="addTab()"
      >
        <PlusIcon />
      </Button>
    </div>

    <!-- One body per tab, all mounted, only the active one shown: that is what
         keeps a Terminal's pty alive across switches (`v-show`, see the note). -->
    <template v-if="tabs.length > 0">
      <div
        v-for="tab in tabs"
        v-show="tab.id === activeTabId"
        :key="tab.id"
        class="flex min-h-0 flex-1 flex-col"
      >
        <DockNode
          v-if="tab.layout"
          :node="tab.layout"
          :tab-id="tab.id"
          :depth="0"
          :active="tab.id === activeTabId"
          :visible="props.visible"
          :workspace="workspace"
        />

        <!-- An empty tab: the chooser. Only enabled tools are offered (S5), and
             when nothing is enabled the tab says where the switch is. -->
        <div
          v-else
          class="flex flex-1 flex-col items-center justify-center gap-2 overflow-y-auto p-4"
        >
          <p class="text-center text-xs font-medium text-muted-foreground">
            {{ t("tools.tabs.chooseTool") }}
          </p>
          <template v-if="available.length > 0">
            <Button
              v-for="tool in available"
              :key="tool.id"
              variant="outline"
              size="xs"
              class="w-full justify-start"
              @click="start(tab.id, tool.id)"
            >
              <component :is="tool.icon" />
              {{ t(tool.labelKey) }}
            </Button>
          </template>
          <p v-else class="text-center text-xs text-muted-foreground">
            {{ t("tools.tabs.noTools") }}
          </p>
        </div>
      </div>
    </template>

    <!-- No tab at all: the drawer's starting face, and what closing the last tab
         returns to. Nothing is opened for the human. -->
    <div
      v-else
      class="flex flex-1 flex-col items-center justify-center gap-2 p-4 text-center"
    >
      <p class="text-xs text-muted-foreground">{{ t("tools.tabs.empty") }}</p>
      <Button variant="outline" size="xs" @click="addTab()">
        <PlusIcon />
        {{ t("tools.tabs.new") }}
      </Button>
    </div>
  </div>
</template>
