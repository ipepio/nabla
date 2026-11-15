<script setup lang="ts">
// App shell (spec S1): a fixed left drawer and a central area that fills the
// rest of the window and *is* the chat.
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

    <!--
      RIGHT DRAWER SEAM (S4) — documented, deliberately not built.

      The layout is `SidebarProvider > [Sidebar][SidebarInset]`, so a future
      right-hand panel is one more sibling that requires no restructuring:

        <Sidebar side="right" collapsible="offcanvas">…</Sidebar>

      `SidebarInset` below keeps `flex-1`, so adding that sibling shrinks the
      chat area instead of overlapping it. Nothing else has to move: the chat
      owns its own scroll and its own width.
    -->
    <SidebarInset class="flex h-svh min-h-0 flex-1 flex-col overflow-hidden">
      <ChatPanel :workspace="activeWorkspace" />
    </SidebarInset>
  </SidebarProvider>
</template>
