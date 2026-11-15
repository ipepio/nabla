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
import { Button } from "./components/ui/button";
import { SidebarInset, SidebarProvider } from "./components/ui/sidebar";
import type { WorkspaceInfo } from "./contracts/types";

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
          RIGHT VIEWPORT (S6, resizable since S3) — still a placeholder, but no
          longer inert: `ResizableAside` owns the drag handle, the pointer and
          keyboard resizing, and the panel width itself (S4: one owner). This
          component keeps only `toolsOpen`, and the panel therefore resets to
          its default width when reopened.

          The user asked for an empty right viewport to be left for future tools;
          nothing inside it is interactive yet. It stays a plain flex sibling, so
          a real tool panel replaces its content without moving the chat: the
          chat owns its own scroll and only loses the width this rail takes.

          When it grows controls, promote it to
          `<Sidebar side="right" collapsible="offcanvas">` inside the provider
          below — that needs no restructuring either.
        -->
        <ResizableAside
          v-if="toolsOpen"
          aria-label="Tools"
          class="hidden md:flex"
        >
          <div class="flex items-center gap-2 border-b px-3 py-2">
            <h2 class="text-sm font-semibold">Tools</h2>
            <Button
              variant="ghost"
              size="icon-sm"
              class="ms-auto"
              aria-label="Collapse tools panel"
              @click="toolsOpen = false"
            >
              <PanelRightCloseIcon />
            </Button>
          </div>
          <div class="flex flex-1 items-center justify-center p-4">
            <p class="text-center text-xs text-muted-foreground">
              No tools yet. This viewport is reserved.
            </p>
          </div>
        </ResizableAside>
      </div>
    </SidebarInset>
  </SidebarProvider>
</template>
