<script setup lang="ts">
// App shell (spec S1): a fixed left drawer and a central area that splits into
// the chat and an empty right viewport (S6).
//
// The drawer owns the workspace and settings data; this component owns only the
// active selection, so ChatPanel receives one workspace instead of the list.
import { nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { PanelRightCloseIcon } from "@lucide/vue";
import AppSidebar from "./components/AppSidebar.vue";
import ChatPanel from "./components/ChatPanel.vue";
import ResizableAside from "./components/ResizableAside.vue";
import ToolTabs from "./components/tools/ToolTabs.vue";
import {
  activateTab,
  addTab,
  setTabTool,
  tabPanes,
  tabs,
} from "./components/tools/dock";
import { Button } from "./components/ui/button";
import { SidebarInset, SidebarProvider } from "./components/ui/sidebar";
import { subscribeEmbedNeedView } from "./contracts/commands";
import type { WorkspaceInfo } from "./contracts/types";
import { t } from "./lib/i18n";
import { isToolEnabled } from "./lib/preferences";

const activeWorkspace = ref<WorkspaceInfo | null>(null);
// Right tools drawer (S6): collapsed by default; the chat header and the empty
// state both expose the toggle, so it is reachable in every state.
const toolsOpen = ref(false);

/**
 * `auto-open-pane.md` (S1, S2, S5): a browser tool needs a view and there is
 * none, so the app opens the pane itself and the agent's call goes through.
 *
 * This handler is the only place that can do it: `toolsOpen` is local to this
 * component, and the rect the view is created with is measured by the pane, never
 * by Rust (S3). Opening AND focusing (S2) is deliberate — the human sees what the
 * agent is doing.
 */
async function openBrowserPane(): Promise<void> {
  // S5: the drawer's tools are the human's switches, and Browser is off by
  // default. Nothing is enabled from behind the counter: the tool answers its
  // honest "no view" error instead, which names this very cause.
  if (!isToolEnabled("browser")) return;

  toolsOpen.value = true;
  // Let the drawer mount BEFORE building the tab: its host reconciles the tabs
  // against the active workspace on mount (`applyDock`), and a tab created for a
  // workspace the drawer has not seen yet is one of the tabs it clears.
  await nextTick();

  // A tab that already holds a browser pane is activated, not duplicated: the
  // pane is a singleton, and only the drawn tab builds a view.
  const existing = tabs.value.find((tab) =>
    tabPanes(tab).some((instance) => instance.toolId === "browser"),
  );
  if (existing !== undefined) {
    activateTab(existing.id);
    return;
  }

  // No browser pane anywhere: one empty tab becomes the browser, and the pane's
  // own active mount creates the view with its measured rect (BrowserTool.vue).
  const tab = addTab();
  setTabTool(tab, "browser", activeWorkspace.value);
}

/** The `embed-need-view` unsubscribe, once the listener is up. */
let unsubscribeNeedView: (() => void) | null = null;
/** Set on unmount: a subscribe that answers afterwards releases itself. */
let disposed = false;

onMounted(async () => {
  const off = await subscribeEmbedNeedView(() => void openBrowserPane());
  if (disposed) {
    off();
    return;
  }
  unsubscribeNeedView = off;
});

onBeforeUnmount(() => {
  disposed = true;
  unsubscribeNeedView?.();
  unsubscribeNeedView = null;
});
</script>

<template>
  <SidebarProvider class="h-svh overflow-hidden">
    <AppSidebar
      :active-workspace-id="activeWorkspace?.id ?? null"
      @select="activeWorkspace = $event"
    />

    <SidebarInset class="h-svh min-h-0 flex-1 overflow-hidden">
      <!--
        Central area is a row: the chat takes the remaining width, the right
        viewport is a resizable rail (default 280px, 240–560px).
      -->
      <div class="flex min-h-0 flex-1 flex-row">
        <div class="flex min-h-0 min-w-0 flex-1 flex-col">
          <ChatPanel
            :workspace="activeWorkspace"
            :tools-open="toolsOpen"
            @toggle-tools="toolsOpen = !toolsOpen"
          />
        </div>

        <!--
          RIGHT VIEWPORT (S6, resizable since S3) — the tools drawer (S2 of
          `tool-drawer-and-mcp.md`). `ResizableAside` owns the drag handle, the
          pointer and keyboard resizing, and the panel width itself (S4: one
          owner); this component keeps only `toolsOpen`, so the panel resets to
          its default width when reopened.

          The body is `ToolTabs`, which hosts the DOCK: a split tree of tool
          instances (`components/tools/dock.ts`) whose "+ Add tool" menu is the
          registry. That is the whole integration: a new tool is a registry
          entry, never an edit here. The dock receives the active workspace,
          which is the default context of a new pane and the reset rule of the
          layout, so a workspace switch never shows the previous folder's panes.
        -->
        <ResizableAside
          v-if="toolsOpen"
          :aria-label="t('tools.title')"
          class="hidden md:flex"
        >
          <div class="flex items-center gap-2 border-b px-3 py-2">
            <h2 class="text-sm font-semibold">{{ t("tools.title") }}</h2>
            <Button
              variant="ghost"
              size="icon-sm"
              class="ms-auto"
              :aria-label="t('tools.collapse')"
              @click="toolsOpen = false"
            >
              <PanelRightCloseIcon />
            </Button>
          </div>

          <ToolTabs :workspace="activeWorkspace" />
        </ResizableAside>
      </div>
    </SidebarInset>
  </SidebarProvider>
</template>
