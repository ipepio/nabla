<script setup lang="ts">
// One workspace row (specs S4-S6 of `odd/tasks/sidebar-rework.md`): the expand
// toggle, the folder, the name over the real path, and the row actions.
//
// Presentation only: every interaction is emitted, so `AppSidebar.vue` keeps the
// single select/add/pin/remove path and this component never touches the
// contracts seam. Colours are semantic tokens only — no hex here.
import {
  ChevronDownIcon,
  ChevronRightIcon,
  FolderIcon,
  MoreHorizontalIcon,
  PencilIcon,
  PinIcon,
  Trash2Icon,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { WorkspaceInfo } from "@/contracts/types";
import { t } from "@/lib/i18n";

defineProps<{
  workspace: WorkspaceInfo;
  /** The workspace the chat is bound to (S5). */
  active: boolean;
  /** Its session list is open (S5). */
  expanded: boolean;
  /** S3: edit mode keeps the row actions visible instead of hover-only. */
  editMode: boolean;
  /** A pin request is in flight for this row, so it cannot double-fire. */
  pinPending: boolean;
}>();

const emit = defineEmits<{
  /** Row click: select the workspace and toggle its session list (S5). */
  (e: "activate"): void;
  (e: "togglePin"): void;
  (e: "edit"): void;
  (e: "remove"): void;
}>();
</script>

<template>
  <div
    class="group/ws-row relative rounded-md"
    :class="active ? 'bg-sidebar-accent text-sidebar-accent-foreground' : ''"
  >
    <!-- Row click is one intent: select + toggle. The actions below are
         siblings, never nested: a button inside a button is invalid HTML. -->
    <button
      type="button"
      class="flex w-full min-w-0 items-center gap-1.5 rounded-md px-2 py-1.5 pr-9 text-left text-sm outline-hidden hover:bg-sidebar-accent/60 focus-visible:ring-2 focus-visible:ring-sidebar-ring group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:px-0"
      :aria-expanded="expanded"
      @click="emit('activate')"
    >
      <!-- S5: the collapse toggle rotates in place (S1 of the reference). -->
      <component
        :is="expanded ? ChevronDownIcon : ChevronRightIcon"
        class="size-3.5 shrink-0 text-muted-foreground group-data-[collapsible=icon]:hidden"
        aria-hidden="true"
      />
      <FolderIcon class="size-4 shrink-0" aria-hidden="true" />

      <span class="flex min-w-0 flex-1 flex-col group-data-[collapsible=icon]:hidden">
        <!-- S4/S6: the alias if the store has one, else the last path segment. -->
        <span class="flex min-w-0 items-center gap-1">
          <span class="truncate leading-5">{{ workspace.name }}</span>
          <PinIcon
            v-if="workspace.pinned"
            class="size-3 shrink-0 fill-current text-primary"
            aria-hidden="true"
          />
        </span>
        <!-- S4: the real path is always visible, muted, under the name. -->
        <span class="truncate text-xs leading-4 text-muted-foreground">
          {{ workspace.path }}
        </span>
      </span>
    </button>

    <!-- Row actions: hover/focus reveal, or always visible in edit mode (S3). -->
    <div
      class="absolute top-1 right-0.5 flex items-center gap-0.5 transition-opacity group-data-[collapsible=icon]:hidden"
      :class="
        editMode
          ? 'opacity-100'
          : 'opacity-0 group-focus-within/ws-row:opacity-100 group-hover/ws-row:opacity-100'
      "
    >
      <Button
        variant="ghost"
        size="icon-xs"
        class="text-muted-foreground"
        :aria-label="
          workspace.pinned
            ? t('workspace.unpin', { name: workspace.name })
            : t('workspace.pin', { name: workspace.name })
        "
        :aria-pressed="workspace.pinned"
        :disabled="pinPending"
        @click="emit('togglePin')"
      >
        <PinIcon :class="workspace.pinned ? 'fill-current' : ''" />
      </Button>

      <!-- S6: the ⋯ menu. Edit opens the placeholder dialog for now; remove is
           the real store call (`remove_workspace`). -->
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button
            variant="ghost"
            size="icon-xs"
            class="text-muted-foreground"
            :aria-label="t('common.moreActions', { name: workspace.name })"
          >
            <MoreHorizontalIcon />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" class="w-44">
          <DropdownMenuItem @select="emit('edit')">
            <PencilIcon />
            {{ t("workspace.edit") }}
          </DropdownMenuItem>
          <DropdownMenuItem variant="destructive" @select="emit('remove')">
            <Trash2Icon />
            {{ t("workspace.remove") }}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  </div>
</template>
