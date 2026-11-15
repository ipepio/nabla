<script setup lang="ts">
// Session list of one expanded workspace (spec S5 of
// `odd/tasks/sidebar-rework.md`, live marking from L4 of
// `odd/tasks/session-lifecycle.md`).
//
// Presentational only: `AppSidebar.vue` owns the fetch, the per-workspace cache
// and the refresh triggers, and this component renders whatever it is handed.
// `liveSessionIds` comes from the runtime through the store, so a row never
// claims a process it cannot verify.
import { MessageSquareIcon } from "@lucide/vue";
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
  /** Id of the session the chat follows, `null` when there is none. */
  activeSessionId: string | null;
  /** L4: ids the runtime reports a live pi process for. */
  liveSessionIds: string[];
}>();

const emit = defineEmits<{ (e: "select", session: SessionSummary): void }>();

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
</script>

<template>
  <SidebarMenuSub>
    <SidebarMenuSubItem v-for="session in sessions" :key="session.id">
      <SidebarMenuSubButton
        as="button"
        type="button"
        size="sm"
        :is-active="String(session.id) === activeSessionId"
        class="w-full"
        @click="emit('select', session)"
      >
        <MessageSquareIcon class="size-3.5 shrink-0" aria-hidden="true" />
        <span class="min-w-0 flex-1 truncate">{{ label(session) }}</span>
        <!-- L4: a running session says so instead of showing its age; the dot
             carries the state, the word carries it to a screen reader too. -->
        <span
          v-if="isRunning(session, liveSessionIds)"
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
    </SidebarMenuSubItem>

    <li v-if="loading" class="px-2 py-1 text-xs text-muted-foreground">Loading…</li>
    <li v-else-if="error" class="px-2 py-1 text-xs text-destructive">{{ error }}</li>
    <li v-else-if="sessions.length === 0" class="px-2 py-1 text-xs text-muted-foreground">
      No sessions yet.
    </li>
  </SidebarMenuSub>
</template>
