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
//     built from the enabled tools (S5), never a hardcoded list. An app-wide tool
//     that already lives in another tab stays in the list and JUMPs to that tab
//     (`startToolInTab`): not offering it made the click look refused, and made a
//     pane the panel did not draw impossible to reach.
//  3. reconciling the model with reality (`applyDock`): a new active workspace
//     clears the tabs, a disabled tool closes its panes;
//  4. the "no tabs" empty state, which is what the drawer shows after the human
//     closes its last tab. The drawer does NOT start there: opening it seeds one
//     empty tab (`openDrawer` in `./dock.ts`), so the tool chooser is the first
//     thing on screen (S1 of `drawer-responsiveness-oauth.md`) and the empty state
//     is the state the human chose, never the startup state.
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
import { ArrowRightIcon, LayoutGridIcon, PlusIcon, XIcon } from "@lucide/vue";
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
  startToolInTab,
  tabHoldingAppWideTool,
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
 * Whether an app-wide tool already has a pane in another tab of this drawer.
 *
 * Read through the dock (`tabHoldingAppWideTool`), never by walking the model
 * here: the dock is the layer that knows which panes exist and which of them this
 * panel draws, and the chooser must not disagree with what `startToolInTab` will
 * do. A tool held in another tab is still offered — clicking it goes there.
 */
function heldElsewhere(tool: ToolDefinition): DockTab | undefined {
  if (tool.appWide !== true) return undefined;
  return tabHoldingAppWideTool(tool.id);
}

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

/** Starts one enabled tool in an empty tab (the dock routes an already-open one). */
function start(tabId: string, toolId: ToolDefinition["id"]): void {
  startToolInTab(tabId, toolId, props.workspace);
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
      class="flex shrink-0 items-center gap-1.5 border-b border-sidebar-border bg-sidebar px-2.5 py-1.5 text-sidebar-foreground"
    >
      <div class="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
        <div
          v-for="tab in tabs"
          :key="tab.id"
          class="group/tab flex min-w-0 shrink-0 items-center rounded-md border transition-colors"
          :class="
            tab.id === activeTabId
              ? 'border-border bg-sidebar-accent text-sidebar-accent-foreground'
              : 'border-transparent text-muted-foreground hover:bg-secondary hover:text-foreground'
          "
        >
          <button
            type="button"
            class="flex min-w-0 max-w-40 items-center gap-1.5 truncate py-1 pl-2 pr-1 text-[11px] outline-hidden focus-visible:ring-2 focus-visible:ring-sidebar-ring"
            :class="tab.id === activeTabId ? 'font-semibold' : 'font-medium'"
            :title="tabLabel(tab)"
            @click="activateTab(tab.id)"
          >
            <component
              :is="tabIcon(tab)"
              v-if="tabIcon(tab) !== null"
              class="size-3.5 shrink-0"
              :class="tab.id === activeTabId ? 'text-primary' : 'text-muted-foreground'"
              aria-hidden="true"
            />
            <span class="truncate">{{ tabLabel(tab) }}</span>
          </button>
          <Button
            variant="ghost"
            size="icon-xs"
            class="me-0.5 size-5 shrink-0 opacity-40 group-hover/tab:opacity-100"
            :class="tab.id === activeTabId ? 'opacity-80' : ''"
            :aria-label="t('tools.tabs.close')"
            :title="t('tools.tabs.close')"
            @click.stop="closeTab(tab.id)"
          >
            <XIcon />
          </Button>
        </div>
      </div>

      <Button
        variant="outline"
        size="icon-xs"
        class="size-7 shrink-0 rounded-md border-border bg-card text-foreground hover:border-primary/40 hover:text-primary"
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
          class="flex flex-1 flex-col items-center justify-center overflow-y-auto p-6"
        >
          <div class="flex w-full max-w-sm flex-col items-center gap-5">
            <div class="flex flex-col items-center gap-2 text-center">
              <div
                class="grid size-10 place-items-center rounded-xl border border-primary/25 bg-primary/10 text-primary"
                aria-hidden="true"
              >
                <LayoutGridIcon class="size-5" />
              </div>
              <h3 class="text-sm font-semibold text-foreground">
                {{ t("tools.tabs.chooseTool") }}
              </h3>
            </div>

            <div
              v-if="available.length > 0"
              class="flex w-full flex-col gap-2"
            >
              <button
                v-for="tool in available"
                :key="tool.id"
                type="button"
                class="group/card flex w-full items-center gap-3 rounded-xl border border-border bg-card p-3 text-left transition-colors outline-hidden hover:border-primary/45 hover:bg-secondary/70 focus-visible:ring-2 focus-visible:ring-ring"
                :title="
                  heldElsewhere(tool) !== undefined
                    ? t('tools.tabs.openElsewhere')
                    : undefined
                "
                @click="start(tab.id, tool.id)"
              >
                <span
                  class="grid size-9 shrink-0 place-items-center rounded-lg border border-border bg-secondary text-primary transition-colors group-hover/card:border-primary/35 group-hover/card:bg-primary/10"
                  aria-hidden="true"
                >
                  <component :is="tool.icon" class="size-4" />
                </span>
                <span class="flex min-w-0 flex-1 flex-col gap-0.5">
                  <span class="flex items-center gap-2">
                    <span class="truncate text-xs font-semibold text-foreground">
                      {{ t(tool.labelKey) }}
                    </span>
                    <span
                      v-if="heldElsewhere(tool) !== undefined"
                      class="shrink-0 rounded border border-tertiary/30 bg-tertiary/10 px-1.5 py-0.5 font-mono text-[9.5px] font-medium text-tertiary"
                    >
                      {{ t("tools.tabs.openElsewhere") }}
                    </span>
                  </span>
                  <span class="line-clamp-2 text-[11px] leading-snug text-muted-foreground">
                    {{ t(tool.descriptionKey) }}
                  </span>
                </span>
                <ArrowRightIcon
                  class="size-3.5 shrink-0 text-muted-foreground/50 transition-colors group-hover/card:text-primary"
                  aria-hidden="true"
                />
              </button>
            </div>
            <p v-else class="max-w-xs text-center text-xs leading-relaxed text-muted-foreground">
              {{ t("tools.tabs.noTools") }}
            </p>
          </div>
        </div>
      </div>
    </template>

    <!-- No tab at all: what closing the last tab returns to. The drawer's first
         open seeds one empty tab instead (`openDrawer`), so this is a state the
         human reached, not where the drawer starts. -->
    <div
      v-else
      class="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center"
    >
      <div
        class="grid size-10 place-items-center rounded-xl border border-border bg-card text-muted-foreground"
        aria-hidden="true"
      >
        <LayoutGridIcon class="size-5" />
      </div>
      <p class="max-w-xs text-xs leading-relaxed text-muted-foreground">
        {{ t("tools.tabs.empty") }}
      </p>
      <Button
        variant="outline"
        size="sm"
        class="gap-1.5 rounded-lg border-border bg-card hover:border-primary/45 hover:text-primary"
        @click="addTab()"
      >
        <PlusIcon class="size-3.5" />
        {{ t("tools.tabs.new") }}
      </Button>
    </div>
  </div>
</template>
