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
//
// H4 of `odd/tasks/subagent-helpers.md`: a chat that delegated shows its children
// nested under it (S2: "no en una lista global"), read from the shared subagent
// store — which `AppSidebar` fills and which owns the interval that keeps a running
// child's state fresh. Clicking one is emitted like every other row: it opens the
// read-only view, never a chat.
import { onMounted, onUnmounted, ref } from "vue";
import {
  ChevronRightIcon,
  GitBranchIcon,
  MessageSquareIcon,
  MoreHorizontalIcon,
  PauseIcon,
  PencilIcon,
  SquareIcon,
  Trash2Icon,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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
import { t } from "@/lib/i18n";
import {
  closeSubagentView,
  isSubagentActive,
  loadSubagentChildren,
  openSubagentId as currentOpenSubagentId,
  subagentChildrenOf,
  subagentStatusDot,
  subagentStatusLabel,
} from "@/lib/subagent-store";
import { cancelSubagent, deleteSession } from "@/contracts/commands";
import type { SessionSummary, SubagentSession } from "@/contracts/types";

/** One child row of the list, with the chat that delegated to it (H4). */
interface OpenSubagentRow {
  child: SubagentSession;
  parent: SessionSummary;
}

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
  /** Id of the child the read-only view shows, `null` when it shows a chat. */
  openSubagentId: number | null;
}>();

const emit = defineEmits<{
  (e: "select", session: SessionSummary): void;
  /** Stop the agent but keep the row, its transcript and its index entry. */
  (e: "pause", session: SessionSummary): void;
  /** Rename the session's title in the index. */
  (e: "rename", payload: { session: SessionSummary; title: string }): void;
  /** Remove the session from the app; the parent archives and deletes it. */
  (e: "delete", session: SessionSummary): void;
  /** Show the read-only view of a child this chat delegated to (H4). */
  (e: "openSubagent", opened: OpenSubagentRow): void;
}>();

/** The session the confirmation dialog is about, `null` when it is closed. */
const pendingDelete = ref<SessionSummary | null>(null);
const deleteOpen = ref(false);
const pendingRename = ref<SessionSummary | null>(null);
const renameDraft = ref("");
const renameOpen = ref(false);

function requestRename(session: SessionSummary) {
  pendingRename.value = session;
  renameDraft.value = session.title ?? "";
  renameOpen.value = true;
}

function confirmRename() {
  const session = pendingRename.value;
  const title = renameDraft.value.trim();
  renameOpen.value = false;
  pendingRename.value = null;
  if (session === null || title === "") return;
  emit("rename", { session, title });
}

/**
 * How often the relative ages are recomputed. The list itself is refetched by
 * the parent; this clock only exists so "now" becomes "1m" and "4h" becomes
 * "5h" without a fetch. One minute is the resolution the labels are written at.
 */
const AGE_TICK_MS = 60_000;
/** Bumped by the clock; reading it in a row is what re-runs that row's label. */
const ageClock = ref(0);
let ageTickHandle: number | null = null;

/** Whether this session currently has a pi process behind it. */
function isRunning(session: SessionSummary, liveSessionIds: string[]): boolean {
  return liveSessionIds.includes(String(session.id));
}

/**
 * The children this chat delegated to (H4), as the shared store knows them. Read
 * inside the render: the rows appear the moment the parent's list lands, and their
 * status dots move on their own while something runs.
 */
function childrenOf(session: SessionSummary): SubagentSession[] {
  return subagentChildrenOf(session.id);
}

function activeChildrenOf(session: SessionSummary): SubagentSession[] {
  return childrenOf(session).filter((child) => isSubagentActive(child.status));
}

function finishedChildrenOf(session: SessionSummary): SubagentSession[] {
  return childrenOf(session).filter((child) => !isSubagentActive(child.status));
}

/** Finished subagent groups stay collapsed by default to keep the sidebar tidy. */
const expandedFinishedGroups = ref<Record<number, boolean>>({});
const deletingFinishedFor = ref<number | null>(null);

function isFinishedGroupOpen(sessionId: number): boolean {
  return expandedFinishedGroups.value[sessionId] === true;
}

function toggleFinishedGroup(sessionId: number): void {
  expandedFinishedGroups.value = {
    ...expandedFinishedGroups.value,
    [sessionId]: !isFinishedGroupOpen(sessionId),
  };
}

function finishedGroupLabel(count: number): string {
  return count === 1
    ? t("chat.subagent.finishedOne")
    : t("chat.subagent.finishedMany", { count });
}

/** What a child row is called: the agent definition it runs. */
function childLabel(child: SubagentSession): string {
  return child.agent.trim() !== "" ? child.agent : child.name;
}

function handleCancelChild(child: SubagentSession): void {
  void cancelSubagent(child.sessionPath).catch(() => {});
}

async function handleDeleteChild(
  child: SubagentSession,
  parent: SessionSummary,
): Promise<void> {
  try {
    if (currentOpenSubagentId.value === child.sessionId) {
      closeSubagentView();
    }
    await deleteSession(child.sessionId);
    await loadSubagentChildren(parent.workspaceId, [parent.id], { force: true });
  } catch {
    // Best-effort: row stays if deletion was rejected.
  }
}

async function handleDeleteAllFinished(parent: SessionSummary): Promise<void> {
  if (deletingFinishedFor.value === parent.id) return;
  const finished = finishedChildrenOf(parent);
  if (finished.length === 0) return;
  deletingFinishedFor.value = parent.id;
  try {
    if (
      currentOpenSubagentId.value !== null &&
      finished.some((child) => child.sessionId === currentOpenSubagentId.value)
    ) {
      closeSubagentView();
    }
    await Promise.allSettled(finished.map((child) => deleteSession(child.sessionId)));
    const next = { ...expandedFinishedGroups.value };
    delete next[parent.id];
    expandedFinishedGroups.value = next;
    await loadSubagentChildren(parent.workspaceId, [parent.id], { force: true });
  } finally {
    deletingFinishedFor.value = null;
  }
}

type RowContextMenu =
  | { kind: "session"; x: number; y: number; session: SessionSummary }
  | { kind: "subagent"; x: number; y: number; child: SubagentSession; parent: SessionSummary }
  | { kind: "finishedGroup"; x: number; y: number; parent: SessionSummary; count: number };

const rowMenu = ref<RowContextMenu | null>(null);

function closeRowMenu(): void {
  rowMenu.value = null;
}

function clampMenuPos(clientX: number, clientY: number): { x: number; y: number } {
  const menuW = 220;
  const menuH = 160;
  return {
    x: Math.min(clientX, Math.max(8, window.innerWidth - menuW - 8)),
    y: Math.min(clientY, Math.max(8, window.innerHeight - menuH - 8)),
  };
}

function openSessionContextMenu(event: MouseEvent, session: SessionSummary): void {
  if (import.meta.env.DEV && event.shiftKey) return;
  event.preventDefault();
  event.stopPropagation();
  const { x, y } = clampMenuPos(event.clientX, event.clientY);
  rowMenu.value = { kind: "session", x, y, session };
}

function openSubagentContextMenu(
  event: MouseEvent,
  child: SubagentSession,
  parent: SessionSummary,
): void {
  if (import.meta.env.DEV && event.shiftKey) return;
  event.preventDefault();
  event.stopPropagation();
  const { x, y } = clampMenuPos(event.clientX, event.clientY);
  rowMenu.value = { kind: "subagent", x, y, child, parent };
}

function openFinishedGroupContextMenu(
  event: MouseEvent,
  parent: SessionSummary,
  count: number,
): void {
  if (import.meta.env.DEV && event.shiftKey) return;
  event.preventDefault();
  event.stopPropagation();
  const { x, y } = clampMenuPos(event.clientX, event.clientY);
  rowMenu.value = { kind: "finishedGroup", x, y, parent, count };
}

function onWindowKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape" && rowMenu.value !== null) {
    closeRowMenu();
  }
}

/** The contract has no title yet, so the fallback is the neutral "Session". */
function label(session: SessionSummary): string {
  return session.title ?? t("session.untitled");
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
  if (minutes < 1) return t("session.age.now");
  if (minutes < 60) return t("session.age.minutes", { count: minutes });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return t("session.age.hours", { count: hours });
  const days = Math.floor(hours / 24);
  if (days < 7) return t("session.age.days", { count: days });
  return new Date(then).toLocaleDateString();
}

/**
 * The age a row shows. Reading `ageClock` here (annotated, not memorised) makes
 * the minute tick a render dependency, so the label ages on its own while
 * `timeLabel` stays a pure function of the stamp.
 */
function rowAgeLabel(updatedAt: string): string {
  void ageClock.value;
  return timeLabel(updatedAt);
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

onMounted(() => {
  ageTickHandle = window.setInterval(() => {
    ageClock.value += 1;
  }, AGE_TICK_MS);
  window.addEventListener("pointerdown", closeRowMenu);
  window.addEventListener("keydown", onWindowKeydown);
  window.addEventListener("blur", closeRowMenu);
});

onUnmounted(() => {
  window.removeEventListener("pointerdown", closeRowMenu);
  window.removeEventListener("keydown", onWindowKeydown);
  window.removeEventListener("blur", closeRowMenu);
  if (ageTickHandle === null) return;
  window.clearInterval(ageTickHandle);
  ageTickHandle = null;
});
</script>

<template>
  <SidebarMenuSub class="mx-0 gap-0.5 border-l-0 px-1.5 pt-0.5 pb-1.5">
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
        class="relative h-auto w-full items-center gap-2.5 rounded-lg border border-transparent px-2.5 py-2 pr-7 text-left text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground data-[active=true]:border-border data-[active=true]:bg-sidebar-accent data-[active=true]:text-foreground data-[active=true]:before:absolute data-[active=true]:before:inset-y-2 data-[active=true]:before:-left-1 data-[active=true]:before:w-0.5 data-[active=true]:before:rounded-full data-[active=true]:before:bg-primary"
        @click="emit('select', session)"
        @contextmenu="openSessionContextMenu($event, session)"
      >
        <MessageSquareIcon
          class="size-4 shrink-0 opacity-75"
          aria-hidden="true"
        />
        <span class="flex min-w-0 flex-1 flex-col gap-0.5">
          <span
            class="truncate text-[12px] leading-tight font-semibold"
            :class="
              String(session.id) === activeSessionId
                ? 'text-foreground'
                : 'text-foreground/85'
            "
          >
            {{ label(session) }}
          </span>
          <span class="truncate text-[10.5px] leading-tight text-muted-foreground">
            <template v-if="busySessionIds.includes(String(session.id))">
              {{ t("session.working") }}
            </template>
            <template v-else-if="isRunning(session, liveSessionIds)">
              {{ t("session.running") }}
            </template>
            <time v-else :datetime="session.updatedAt" class="tabular-nums">
              {{ rowAgeLabel(session.updatedAt) }}
            </time>
          </span>
        </span>
        <!-- Right-aligned status dot (fades out on hover when row actions appear). -->
        <span
          v-if="busySessionIds.includes(String(session.id))"
          class="nabla-status-pulse size-2 shrink-0 rounded-full bg-primary transition-opacity group-focus-within/session-row:opacity-0 group-hover/session-row:opacity-0"
          role="status"
          :aria-label="t('session.working')"
          :title="t('session.working')"
        />
        <span
          v-else-if="isRunning(session, liveSessionIds)"
          class="size-2 shrink-0 rounded-full bg-primary transition-opacity group-focus-within/session-row:opacity-0 group-hover/session-row:opacity-0"
          :title="t('session.runningTooltip')"
          aria-hidden="true"
        />
        <span
          v-else
          class="size-2 shrink-0 rounded-full bg-tertiary/80 transition-opacity group-focus-within/session-row:opacity-0 group-hover/session-row:opacity-0"
          aria-hidden="true"
        />
      </SidebarMenuSubButton>

      <!-- Row actions: hover/focus reveal, aligned with the session card. The
           button is a sibling of the row (never nested: a button inside a
           button is invalid HTML) and keeps keyboard reach through focus-within. -->
      <div
        class="absolute top-2.5 right-1 opacity-0 transition-opacity group-focus-within/session-row:opacity-100 group-hover/session-row:opacity-100"
      >
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button
              variant="ghost"
              size="icon-xs"
              class="text-muted-foreground hover:bg-accent hover:text-foreground"
              :aria-label="t('common.moreActions', { name: label(session) })"
            >
              <MoreHorizontalIcon />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" class="w-44">
            <DropdownMenuItem @select="requestRename(session)">
              <PencilIcon />
              {{ t("session.rename") }}
            </DropdownMenuItem>
            <!-- Pause keeps the row, its transcript and its index entry: it only
                 stops the process, so it is offered only when one is live. -->
            <DropdownMenuItem
              :disabled="!isRunning(session, liveSessionIds)"
              @select="emit('pause', session)"
            >
              <PauseIcon />
              {{ t("session.pause") }}
            </DropdownMenuItem>
            <DropdownMenuItem variant="destructive" @select="requestDelete(session)">
              <Trash2Icon />
              {{ t("common.delete") }}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <!-- H4: the chats this one delegated to, nested under it (S2). Indented with
           a vertical tree line and no ⋯ menu: a child is read, never paused or
           deleted from here — its life belongs to the parent chat that started it.
           Clicking one opens the read-only view, not a conversation. -->
      <ul
        v-if="childrenOf(session).length > 0"
        class="mt-1 mb-1.5 ml-4 flex flex-col gap-0.5 border-l border-border pl-2.5"
        :aria-label="t('chat.subagent.list')"
      >
        <li
          v-for="child in activeChildrenOf(session)"
          :key="child.sessionId"
          class="group/subagent-row relative"
        >
          <SidebarMenuSubButton
            as="button"
            type="button"
            size="sm"
            :is-active="child.sessionId === openSubagentId"
            class="h-auto w-full items-center gap-2.5 rounded-lg border border-transparent px-2.5 py-2 text-left text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground data-[active=true]:border-border data-[active=true]:bg-sidebar-accent data-[active=true]:text-foreground"
            @click="emit('openSubagent', { child, parent: session })"
            @contextmenu="openSubagentContextMenu($event, child, session)"
          >
            <GitBranchIcon
              class="size-3.5 shrink-0"
              :class="
                child.sessionId === openSubagentId
                  ? 'text-primary'
                  : 'text-muted-foreground'
              "
              aria-hidden="true"
            />
            <span class="flex min-w-0 flex-1 flex-col gap-0.5">
              <span
                class="truncate text-[12px] leading-tight font-semibold"
                :class="
                  child.sessionId === openSubagentId
                    ? 'text-foreground'
                    : 'text-foreground/85'
                "
              >
                {{ childLabel(child) }}
              </span>
              <span class="truncate text-[10.5px] leading-tight text-muted-foreground">
                {{ subagentStatusLabel(child.status) }} · {{ t("chat.subagent.readOnly") }}
              </span>
            </span>
            <span
              class="size-2 shrink-0 rounded-full transition-opacity group-hover/subagent-row:opacity-0"
              :class="subagentStatusDot(child.status)"
              :title="subagentStatusLabel(child.status)"
              aria-hidden="true"
            />
          </SidebarMenuSubButton>
          <button
            type="button"
            class="absolute top-1/2 right-1.5 flex size-6 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground opacity-0 transition-opacity hover:bg-destructive/15 hover:text-destructive group-hover/subagent-row:opacity-100 focus-visible:opacity-100"
            :aria-label="t('chat.subagent.cancel')"
            :title="t('chat.subagent.cancel')"
            @click.stop="handleCancelChild(child)"
          >
            <SquareIcon class="size-3 fill-current" />
          </button>
        </li>

        <li
          v-if="finishedChildrenOf(session).length > 0"
          class="group/finished-group relative"
        >
          <div class="relative flex items-center">
            <button
              type="button"
              class="flex w-full items-center gap-1.5 rounded-lg px-2 py-1.5 pr-7 text-left text-[11px] font-medium text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
              :aria-expanded="isFinishedGroupOpen(session.id)"
              @click="toggleFinishedGroup(session.id)"
              @contextmenu="openFinishedGroupContextMenu($event, session, finishedChildrenOf(session).length)"
            >
              <ChevronRightIcon
                class="size-3.5 shrink-0 transition-transform"
                :class="isFinishedGroupOpen(session.id) ? 'rotate-90' : ''"
                aria-hidden="true"
              />
              <span class="min-w-0 flex-1 truncate">
                {{ finishedGroupLabel(finishedChildrenOf(session).length) }}
              </span>
            </button>
            <button
              type="button"
              :disabled="deletingFinishedFor === session.id"
              class="absolute top-1/2 right-1 flex size-6 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground opacity-0 transition-opacity hover:bg-destructive/15 hover:text-destructive group-hover/finished-group:opacity-100 focus-visible:opacity-100 disabled:opacity-40"
              :aria-label="t('chat.subagent.deleteAllFinishedCount', { count: finishedChildrenOf(session).length })"
              :title="t('chat.subagent.deleteAllFinishedCount', { count: finishedChildrenOf(session).length })"
              @click.stop="void handleDeleteAllFinished(session)"
            >
              <Trash2Icon class="size-3.5" />
            </button>
          </div>

          <ul
            v-if="isFinishedGroupOpen(session.id)"
            class="mt-0.5 flex flex-col gap-0.5 pl-2"
          >
            <li
              v-for="child in finishedChildrenOf(session)"
              :key="child.sessionId"
              class="group/subagent-row relative"
            >
              <SidebarMenuSubButton
                as="button"
                type="button"
                size="sm"
                :is-active="child.sessionId === openSubagentId"
                class="h-auto w-full items-center gap-2.5 rounded-lg border border-transparent px-2.5 py-1.5 text-left text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground data-[active=true]:border-border data-[active=true]:bg-sidebar-accent data-[active=true]:text-foreground"
                @click="emit('openSubagent', { child, parent: session })"
                @contextmenu="openSubagentContextMenu($event, child, session)"
              >
                <GitBranchIcon
                  class="size-3.5 shrink-0"
                  :class="
                    child.sessionId === openSubagentId
                      ? 'text-primary'
                      : 'text-muted-foreground'
                  "
                  aria-hidden="true"
                />
                <span class="flex min-w-0 flex-1 flex-col gap-0.5">
                  <span
                    class="truncate text-[12px] leading-tight font-semibold"
                    :class="
                      child.sessionId === openSubagentId
                        ? 'text-foreground'
                        : 'text-foreground/85'
                    "
                  >
                    {{ childLabel(child) }}
                  </span>
                  <span class="truncate text-[10.5px] leading-tight text-muted-foreground">
                    {{ subagentStatusLabel(child.status) }} · {{ t("chat.subagent.readOnly") }}
                  </span>
                </span>
                <span
                  class="size-2 shrink-0 rounded-full"
                  :class="subagentStatusDot(child.status)"
                  :title="subagentStatusLabel(child.status)"
                  aria-hidden="true"
                />
              </SidebarMenuSubButton>
            </li>
          </ul>
        </li>
      </ul>
    </SidebarMenuSubItem>

    <li v-if="loading" class="px-2 py-1 text-xs text-muted-foreground">
      {{ t("common.loading") }}
    </li>
    <li v-if="actionError" class="px-2 py-1 text-xs text-destructive">{{ actionError }}</li>
    <template v-if="!loading">
      <li v-if="error" class="px-2 py-1 text-xs text-destructive">{{ error }}</li>
      <li v-else-if="sessions.length === 0" class="px-2 py-1 text-xs text-muted-foreground">
        {{ t("session.empty") }}
      </li>
    </template>

    <!-- One dialog for the whole list: the trigger only says which row it is
         about, so the list never renders one dialog per session. -->
    <Teleport to="body">
      <div
        v-if="rowMenu"
        role="menu"
        class="fixed z-50 w-56 overflow-hidden rounded-xl border border-border bg-popover p-1 text-popover-foreground shadow-xl"
        :style="{ left: `${rowMenu.x}px`, top: `${rowMenu.y}px` }"
        @pointerdown.stop
        @contextmenu.prevent
      >
        <template v-if="rowMenu.kind === 'session'">
          <button
            type="button"
            role="menuitem"
            class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
            @click="requestRename(rowMenu.session); closeRowMenu()"
          >
            <PencilIcon class="size-3.5 text-muted-foreground" />
            {{ t("session.rename") }}
          </button>
          <button
            v-if="isRunning(rowMenu.session, liveSessionIds)"
            type="button"
            role="menuitem"
            class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
            @click="emit('pause', rowMenu.session); closeRowMenu()"
          >
            <PauseIcon class="size-3.5 text-muted-foreground" />
            {{ t("session.pause") }}
          </button>
          <div class="my-1 h-px bg-border" role="separator" />
          <button
            type="button"
            role="menuitem"
            class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs text-destructive hover:bg-destructive/10"
            @click="requestDelete(rowMenu.session); closeRowMenu()"
          >
            <Trash2Icon class="size-3.5" />
            {{ t("common.delete") }}
          </button>
        </template>

        <template v-else-if="rowMenu.kind === 'subagent'">
          <button
            type="button"
            role="menuitem"
            class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
            @click="emit('openSubagent', { child: rowMenu.child, parent: rowMenu.parent }); closeRowMenu()"
          >
            <GitBranchIcon class="size-3.5 text-muted-foreground" />
            {{ t("chat.subagent.open") }}
          </button>
          <button
            v-if="isSubagentActive(rowMenu.child.status)"
            type="button"
            role="menuitem"
            class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs text-destructive hover:bg-destructive/10"
            @click="handleCancelChild(rowMenu.child); closeRowMenu()"
          >
            <SquareIcon class="size-3 fill-current" />
            {{ t("chat.subagent.cancel") }}
          </button>
          <div class="my-1 h-px bg-border" role="separator" />
          <button
            type="button"
            role="menuitem"
            class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs text-destructive hover:bg-destructive/10"
            @click="void handleDeleteChild(rowMenu.child, rowMenu.parent); closeRowMenu()"
          >
            <Trash2Icon class="size-3.5" />
            {{ t("common.delete") }}
          </button>
        </template>

        <template v-else>
          <button
            type="button"
            role="menuitem"
            class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
            @click="toggleFinishedGroup(rowMenu.parent.id); closeRowMenu()"
          >
            <ChevronRightIcon
              class="size-3.5 text-muted-foreground transition-transform"
              :class="isFinishedGroupOpen(rowMenu.parent.id) ? 'rotate-90' : ''"
            />
            {{
              isFinishedGroupOpen(rowMenu.parent.id)
                ? t("chat.subagent.collapseFinished")
                : t("chat.subagent.expandFinished")
            }}
          </button>
          <div class="my-1 h-px bg-border" role="separator" />
          <button
            type="button"
            role="menuitem"
            class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs text-destructive hover:bg-destructive/10"
            @click="void handleDeleteAllFinished(rowMenu.parent); closeRowMenu()"
          >
            <Trash2Icon class="size-3.5" />
            {{ t("chat.subagent.deleteAllFinishedCount", { count: rowMenu.count }) }}
          </button>
        </template>
      </div>
    </Teleport>

    <Dialog v-model:open="renameOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{{ t("session.rename.title") }}</DialogTitle>
        </DialogHeader>
        <form class="grid gap-3" @submit.prevent="confirmRename">
          <Input
            v-model="renameDraft"
            type="text"
            autocomplete="off"
            :placeholder="t('session.rename.placeholder')"
            class="h-9 text-sm"
          />
          <DialogFooter>
            <DialogClose as-child>
              <Button type="button" variant="outline">
                {{ t("common.cancel") }}
              </Button>
            </DialogClose>
            <Button type="submit" :disabled="!renameDraft.trim()">
              {{ t("settings.subagents.save") }}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>

    <Dialog v-model:open="deleteOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{{ t("session.delete.title") }}</DialogTitle>
          <DialogDescription>
            {{ t("session.delete.description") }}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <DialogClose as-child>
            <Button variant="outline">{{ t("common.cancel") }}</Button>
          </DialogClose>
          <Button variant="destructive" @click="confirmDelete">
            {{ t("common.delete") }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </SidebarMenuSub>
</template>
