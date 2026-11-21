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
import { openDrawer, openTool } from "./components/tools/dock";
import { Button } from "./components/ui/button";
import { SidebarInset, SidebarProvider } from "./components/ui/sidebar";
import { subscribeEmbedNeedView } from "./contracts/commands";
import type { WorkspaceInfo } from "./contracts/types";
import { t } from "./lib/i18n";
import { enabledTools, isToolEnabled } from "./lib/preferences";

const activeWorkspace = ref<WorkspaceInfo | null>(null);
// Right tools drawer (S6): collapsed by default; the chat header and the empty
// state both expose the toggle, so it is reachable in every state.
const toolsOpen = ref(false);

/**
 * S1 of `odd/tasks/drawer-responsiveness-oauth.md`: opening the drawer is the
 * whole gesture. The dock seeds one EMPTY tab on that open, so the human lands on
 * the tool chooser instead of having to press `+` and then pick a tool.
 *
 * The seed belongs to the OPEN, not to the drawer's reconcile: a human who closes
 * the last tab while the drawer is on screen keeps an empty drawer, and a
 * workspace or settings change that lands afterwards does not put a tab back. The
 * tabs that do exist are untouched, so collapsing and reopening keeps the tab
 * that was active (`dock.ts`).
 */
function toggleDrawer(): void {
  toolsOpen.value = !toolsOpen.value;
  if (toolsOpen.value) {
    openDrawer(activeWorkspace.value, enabledTools.value);
  }
}

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
  // Let the drawer become visible BEFORE building the tab: the pane measures its
  // own rect on mount, and an element that is still hidden measures nothing.
  await nextTick();

  // One call, in the dock, does the whole thing: it reconciles the layout for the
  // workspace the tab is built for (so the reconcile a settling workspace or a
  // settings list triggers a moment later finds the drawer already current and
  // cannot wipe the tab), then reveals the existing browser pane or creates it.
  // That ordering is the fix for the reported bug — the drawer opened with no tab
  // because this write used to land in a layout the next reconcile discarded.
  openTool("browser", activeWorkspace.value, enabledTools.value);
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
            @toggle-tools="toggleDrawer"
          />
        </div>

        <!--
          RIGHT VIEWPORT (S6, resizable since S3) — the tools drawer (S2 of
          `tool-drawer-and-mcp.md`). `ResizableAside` owns the drag handle, the
          pointer and keyboard resizing, and the panel width itself (S4: one
          owner); this component keeps only `toolsOpen`.

          THE DRAWER STAYS MOUNTED (`v-show`, never `v-if`). Collapsing it must not
          restart what lives inside: a Terminal holds its pty and a Browser holds a
          child webview, and unmounting threw both away on every close — the
          reported bug ("al cerrar el drawer y volverlo a abrir, se reinician las
          aplicaciones"). Keeping it mounted has two consequences the rest of this
          file and `BrowserTool.vue` respect:

            - the width is NOT recomputed on reopen: `ResizableAside` measured it
              once, at mount, and the human's drag survives the round trip;
            - `v-show` writes an inline `display`, which is why the utility classes
              below can stay (`hidden md:flex` re-applies as soon as the inline
              value goes back to empty), and why the browser pane needs its own
              native hide: a child webview floats above the HTML and ignores CSS.

          The body is `ToolTabs`, which hosts the DOCK: a split tree of tool
          instances (`components/tools/dock.ts`) whose "+ Add tool" menu is the
          registry. That is the whole integration: a new tool is a registry
          entry, never an edit here. The dock receives the active workspace,
          which is the default context of a new pane and the reset rule of the
          layout, so a workspace switch never shows the previous folder's panes;
          it also receives `visible`, the one fact a child webview cannot learn
          from CSS.
        -->
        <ResizableAside
          v-show="toolsOpen"
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

          <ToolTabs :workspace="activeWorkspace" :visible="toolsOpen" />
        </ResizableAside>
      </div>
    </SidebarInset>
  </SidebarProvider>
</template>
