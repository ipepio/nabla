<script setup lang="ts">
// App shell (spec S1): a fixed left drawer and a central area that splits into
// the chat and an empty right viewport (S6).
//
// The drawer owns the workspace and settings data; this component owns only the
// active selection, so ChatPanel receives one workspace instead of the list.
import { ref } from "vue";
import AppSidebar from "./components/AppSidebar.vue";
import ChatPanel from "./components/ChatPanel.vue";
import { SidebarInset, SidebarProvider } from "./components/ui/sidebar";
import type { WorkspaceInfo } from "./contracts/types";

const activeWorkspace = ref<WorkspaceInfo | null>(null);
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
        viewport keeps a fixed 280px rail.
      -->
      <div class="flex min-h-0 flex-1 flex-row">
        <div class="flex min-h-0 min-w-0 flex-1 flex-col">
          <ChatPanel :workspace="activeWorkspace" />
        </div>

        <!--
          RIGHT VIEWPORT SEAM (S6) — a placeholder, deliberately inert.

          The user asked for an empty right viewport to be left for future tools;
          nothing in it is interactive yet. It is a plain flex sibling, so a real
          tool panel replaces its content without moving the chat: the chat owns
          its own scroll and only loses the width this rail takes.

          When it grows controls, promote it to
          `<Sidebar side="right" collapsible="offcanvas">` inside the provider
          below — that needs no restructuring either.
        -->
        <aside
          aria-label="Tools"
          class="hidden w-[280px] shrink-0 flex-col border-l bg-sidebar md:flex"
        >
          <div class="flex items-center border-b px-3 py-2">
            <h2 class="text-sm font-semibold">Tools</h2>
          </div>
          <div class="flex flex-1 items-center justify-center p-4">
            <p class="text-center text-xs text-muted-foreground">
              No tools yet. This viewport is reserved.
            </p>
          </div>
        </aside>
      </div>
    </SidebarInset>
  </SidebarProvider>
</template>
