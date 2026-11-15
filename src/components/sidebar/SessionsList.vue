<script setup lang="ts">
// Session list of one expanded workspace (spec S5 of
// `odd/tasks/sidebar-rework.md`, live marking from L4 of
// `odd/tasks/session-lifecycle.md`).
//
// Presentational only: `AppSidebar.vue` owns the fetch, the per-workspace cache
// and the refresh triggers, and this component renders whatever it is handed.
// `liveSessionIds` comes from the runtime through the store, so a row never
// claims a process it cannot verify.
//
// Each row carries its own ⋯ menu (pause / delete). Pause is offered only while
// the session has a live process — there is nothing to stop otherwise — and
// delete asks for confirmation before it emits, because a deleted session leaves
// the list. Both actions are emitted: this component never calls a command.
import { ref } from "vue";
import { MessageSquareIcon, MoreHorizontalIcon, PauseIcon, Trash2Icon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
} from "@/components/ui/sidebar";
import type { SessionSummary } from "@/contracts/types";

defineProps<{
  sessions: SessionSummary[];
  /** The fetch for this workspace is in flight. */
  loading: boolean;
  /** The last fetch failed; the message is rendered in place of the rows. */
  error: string | null;
  /** A row action (pause, delete) failed; the message is rendered under the rows. */
  actionError: string | null;
  /** Id of the session the chat follows, `null` when there is none. */
  activeSessionId: string | null;
  /** L4: ids the runtime reports a live pi process for. */
  liveSessionIds: string[];
  /** Ids whose agent is producing output right now (spinner on the row). */
  busySessionIds: string[];
}>();

const emit = defineEmits<{
  (e: "select", session: SessionSummary): void;
  /** Stop the agent but keep the row, its transcript and its index entry. */
  (e: "pause", session: SessionSummary): void;
  /** Remove the session from the app; the parent archives and deletes it. */
  (e: "delete", session: SessionSummary): void;
}>();

/** The session the confirmation dialog is about, `null` when it is closed. */
const pendingDelete = ref<SessionSummary | null>(null);
const deleteOpen = ref(false);

/** Whether this session currently has a pi process behind it. */
function isRunning(session: SessionSummary, liveSessionIds: string[]): boolean {
  return liveSessionIds.includes(String(session.id));
}

/** The contract has no title yet, so the fallback is the neutral "Session". */
function label(session: SessionSummary): string {
  return session.title ?? "Session";
}

/**
 * Right-aligned age of the session, from `updatedAt`: minutes and hours while
 * it is fresh, days inside the week, and a short date beyond that. A session
 * with an unparseable stamp shows nothing rather than "Invalid Date".
 */
function timeLabel(updatedAt: string): string {
  const then = new Date(updatedAt).getTime();
  if (Number.isNaN(then)) return "";
  const minutes = Math.floor((Date.now() - then) / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d`;
  return new Date(then).toLocaleDateString();
}

/** Delete is destructive and permanent in the list, so it always asks first. */
function requestDelete(session: SessionSummary) {
  pendingDelete.value = session;
  deleteOpen.value = true;
}

function confirmDelete() {
  const session = pendingDelete.value;
  deleteOpen.value = false;
  pendingDelete.value = null;
  if (session === null) return;
  emit("delete", session);
}
</script>

<template>
  <SidebarMenuSub class="mx-0 border-l-0 pl-0">
    <SidebarMenuSubItem
      v-for="session in sessions"
      :key="session.id"
      class="group/session-row relative"
    >
      <SidebarMenuSubButton
        as="button"
        type="button"
        size="sm"
        :is-active="String(session.id) === activeSessionId"
        class="w-full pl-2 pr-6"
        @click="emit('select', session)"
      >
        <MessageSquareIcon class="size-3.5 shrink-0" aria-hidden="true" />
        <span class="min-w-0 flex-1 truncate">{{ label(session) }}</span>
        <!-- L4: a running session says so instead of showing its age; the dot
             carries the state, the word carries it to a screen reader too. -->
        <!-- Busy agent: a spinning ring, the same affordance a terminal-style
             harness uses while a turn runs. Live-but-idle keeps the dot. -->
        <span
          v-if="busySessionIds.includes(String(session.id))"
          class="size-3 shrink-0 animate-spin rounded-full border-2 border-warning/30 border-t-warning"
          role="status"
          aria-label="Agent is working"
          title="Agent is working"
        />
        <span
          v-else-if="isRunning(session, liveSessionIds)"
          class="flex shrink-0 items-center gap-1 text-[10px] text-muted-foreground"
          title="pi is running"
        >
          <span class="size-1.5 rounded-full bg-success" aria-hidden="true" />
          <span>running</span>
        </span>
        <time
          v-else
          :datetime="session.updatedAt"
          class="shrink-0 text-[10px] tabular-nums text-muted-foreground"
        >
          {{ timeLabel(session.updatedAt) }}
        </time>
      </SidebarMenuSubButton>

      <!-- Row actions: hover/focus reveal, next to the age label. The button is
           a sibling of the row (never nested: a button inside a button is
           invalid HTML) and keeps keyboard reach through focus-within. -->
      <div
        class="absolute top-1/2 right-0.5 -translate-y-1/2 opacity-0 transition-opacity group-focus-within/session-row:opacity-100 group-hover/session-row:opacity-100"
      >
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button
              variant="ghost"
              size="icon-xs"
              class="text-muted-foreground"
              :aria-label="`More actions for ${label(session)}`"
            >
              <MoreHorizontalIcon />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" class="w-44">
            <!-- Pause keeps the row, its transcript and its index entry: it only
                 stops the process, so it is offered only when one is live. -->
            <DropdownMenuItem
              :disabled="!isRunning(session, liveSessionIds)"
              @select="emit('pause', session)"
            >
              <PauseIcon />
              Pause
            </DropdownMenuItem>
            <DropdownMenuItem variant="destructive" @select="requestDelete(session)">
              <Trash2Icon />
              Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </SidebarMenuSubItem>

    <li v-if="loading" class="px-2 py-1 text-xs text-muted-foreground">Loading…</li>
    <li v-if="actionError" class="px-2 py-1 text-xs text-destructive">{{ actionError }}</li>
    <template v-if="!loading">
      <li v-if="error" class="px-2 py-1 text-xs text-destructive">{{ error }}</li>
      <li v-else-if="sessions.length === 0" class="px-2 py-1 text-xs text-muted-foreground">
        No sessions yet.
      </li>
    </template>

    <!-- One dialog for the whole list: the trigger only says which row it is
         about, so the list never renders one dialog per session. -->
    <Dialog v-model:open="deleteOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Delete this session?</DialogTitle>
          <DialogDescription>
            Its transcript moves to the archive folder and it disappears from the
            list.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <DialogClose as-child>
            <Button variant="outline">Cancel</Button>
          </DialogClose>
          <Button variant="destructive" @click="confirmDelete">Delete</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </SidebarMenuSub>
</template>
