<script setup lang="ts">
// App shell (spec S1): a fixed left drawer and a central area that splits into
// the chat and an empty right viewport (S6).
//
// The drawer owns the workspace and settings data; this component owns only the
// active selection, so ChatPanel receives one workspace instead of the list.
import { ref } from "vue";
import { PanelRightCloseIcon } from "@lucide/vue";
import AppSidebar from "./components/AppSidebar.vue";
import ChatPanel from "./components/ChatPanel.vue";
import ResizableAside from "./components/ResizableAside.vue";
import ToolTabs from "./components/tools/ToolTabs.vue";
import { Button } from "./components/ui/button";
import { SidebarInset, SidebarProvider } from "./components/ui/sidebar";
import type { WorkspaceInfo } from "./contracts/types";
import { t } from "./lib/i18n";

const activeWorkspace = ref<WorkspaceInfo | null>(null);
// Right tools drawer (S6): collapsed by default; the chat header and the empty
// state both expose the toggle, so it is reachable in every state.
const toolsOpen = ref(false);
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
