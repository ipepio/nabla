<script setup lang="ts">
// App shell (spec S1): a fixed left drawer and a central area that splits into
// the chat and an empty right viewport (S6).
//
// The drawer owns the workspace and settings data; this component owns only the
// active selection, so ChatPanel receives one workspace instead of the list.
import { defineAsyncComponent, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { PanelRightCloseIcon, RotateCcwIcon } from "@lucide/vue";
import AppSidebar from "./components/AppSidebar.vue";
import ChatPanel from "./components/ChatPanel.vue";
import ResizableAside from "./components/ResizableAside.vue";
import ToolTabs from "./components/tools/ToolTabs.vue";
import { openDocumentInDrawer, openDrawer, openTool } from "./components/tools/dock";
import { Button } from "./components/ui/button";
import { SidebarInset, SidebarProvider } from "./components/ui/sidebar";
import { subscribeDocOpen, subscribeEmbedNeedView } from "./contracts/commands";
import type { SessionSummary, WorkspaceInfo } from "./contracts/types";
import { setWebviewZoom } from "./contracts/window";
import { t } from "./lib/i18n";
import { enabledTools, isToolEnabled } from "./lib/preferences";
import { selectSession } from "./lib/session-store";
import {
  applyDocumentZoom,
  createZoomController,
  detectPlatform,
  zoomAction,
} from "./lib/zoom";

const CommandPalette = defineAsyncComponent(
  () => import("./components/CommandPalette.vue"),
);

const sidebarRef = ref<{
  openSettings: () => void;
  handleNewSession: () => Promise<void>;
} | null>(null);
const paletteOpen = ref(false);
const paletteLoaded = ref(false);

function openPalette(): void {
  paletteLoaded.value = true;
  paletteOpen.value = true;
}

function handlePaletteSelectWorkspace(workspace: WorkspaceInfo): void {
  activeWorkspace.value = workspace;
}

async function handlePaletteSelectSession(payload: {
  workspace: WorkspaceInfo;
  session: SessionSummary;
}): Promise<void> {
  activeWorkspace.value = payload.workspace;
  await selectSession(payload.session);
}

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

/** Auto-opens the right drawer on the Document tool when an agent opens/exports a doc. */
async function openDocumentPane(path: string): Promise<void> {
  if (!isToolEnabled("document")) return;
  toolsOpen.value = true;
  await nextTick();
  openDocumentInDrawer(path, activeWorkspace.value, enabledTools.value);
}

/** The `embed-need-view` and `doc-open` unsubscribes, once the listeners are up. */
let unsubscribeNeedView: (() => void) | null = null;
let unsubscribeDocOpen: (() => void) | null = null;
/** Set on unmount: a subscribe that answers afterwards releases itself. */
let disposed = false;

/**
 * Zoom (`⌘`/`Ctrl` `+`, `-`, `0`; the whole policy is `src/lib/zoom.ts`).
 *
 * The listener is document-level and lives in the shell because zoom is not a
 * panel's concern: whichever pane has focus — the composer, a terminal, a dialog
 * — the chord is the app's. The platform is read once, from the webview's user
 * agent: macOS takes `⌘`, Windows and Linux take `Ctrl`.
 *
 * The native port is the main webview's page zoom; the document-level fallback is
 * only reached when that refuses (a plain browser during `vite dev`, or a build
 * without it). A chord we do not own is never `preventDefault`ed, so typing
 * `=` or `-` stays typing.
 */
const zoomPlatform = detectPlatform(navigator.userAgent, navigator.platform);
const zoom = createZoomController({
  setNative: (scale) => setWebviewZoom(scale),
  setDocument: (scale) => applyDocumentZoom(document.documentElement, scale),
});

function onZoomKeydown(event: KeyboardEvent): void {
  const mod = zoomPlatform === "mac" ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (mod && !event.altKey) {
    const key = event.key.toLowerCase();
    if (key === "k") {
      event.preventDefault();
      if (paletteOpen.value) {
        paletteOpen.value = false;
      } else {
        openPalette();
      }
      return;
    }
    if (key === "n" && !event.shiftKey) {
      event.preventDefault();
      void sidebarRef.value?.handleNewSession();
      return;
    }
    if (event.key === ".") {
      event.preventDefault();
      toggleDrawer();
      return;
    }
    if (event.key === ",") {
      event.preventDefault();
      sidebarRef.value?.openSettings();
      return;
    }
  }
  const action = zoomAction(event, zoomPlatform);
  if (action === null) return;
  event.preventDefault();
  void zoom.apply(action);
}

onMounted(async () => {
  document.addEventListener("keydown", onZoomKeydown);
  const [offNeedView, offDocOpen] = await Promise.all([
    subscribeEmbedNeedView(() => void openBrowserPane()),
    subscribeDocOpen(({ path }) => void openDocumentPane(path)),
  ]);
  if (disposed) {
    offNeedView();
    offDocOpen();
    return;
  }
  unsubscribeNeedView = offNeedView;
  unsubscribeDocOpen = offDocOpen;
});

onBeforeUnmount(() => {
  document.removeEventListener("keydown", onZoomKeydown);
  disposed = true;
  unsubscribeNeedView?.();
  unsubscribeNeedView = null;
  unsubscribeDocOpen?.();
  unsubscribeDocOpen = null;
});
</script>

<template>
  <SidebarProvider class="h-svh overflow-hidden">
    <AppSidebar
      ref="sidebarRef"
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
            @open-search="openPalette"
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
          v-slot="{ width, resetWidth }"
          :aria-label="t('tools.title')"
          class="hidden md:flex"
        >
          <div class="flex h-13 shrink-0 items-center gap-2 border-b border-sidebar-border px-3.5">
            <h2 class="text-[10px] font-bold tracking-widest text-muted-foreground uppercase">
              {{ t("tools.title") }}
            </h2>
            <span class="ms-auto font-mono text-[10px] tabular-nums text-muted-foreground">
              {{ width }} px
            </span>
            <Button
              variant="ghost"
              size="icon-xs"
              class="text-muted-foreground hover:bg-accent hover:text-foreground"
              aria-label="Reset panel width"
              @click="resetWidth"
            >
              <RotateCcwIcon />
            </Button>
            <Button
              variant="ghost"
              size="icon-xs"
              class="text-muted-foreground hover:bg-accent hover:text-foreground"
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

    <CommandPalette
      v-if="paletteLoaded"
      v-model:open="paletteOpen"
      @select-workspace="handlePaletteSelectWorkspace"
      @select-session="handlePaletteSelectSession"
      @new-session="void sidebarRef?.handleNewSession()"
      @toggle-tools="toggleDrawer"
      @open-settings="sidebarRef?.openSettings()"
    />
  </SidebarProvider>
</template>
