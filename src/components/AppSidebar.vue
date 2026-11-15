<script setup lang="ts">
// Left drawer (specs S1-S6 of `odd/tasks/sidebar-rework.md`): the Nabla logo and
// the collapse control (the only one in the shell), the "New session" button
// (S2), the Workspaces header with its three actions (S3), the collapsible
// workspace list with each workspace's sessions (S4-S6) and the settings dialog
// in the footer.
//
// It owns the workspace/settings data flow and the session *index* it shows; the
// selection is lifted to App.vue through `select`, which feeds ChatPanel, and the
// active session lives in the shared `src/lib/session-store.ts` (task N1).
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import {
  CheckIcon,
  CirclePlusIcon,
  FolderPlusIcon,
  PanelLeftIcon,
  SearchIcon,
  SettingsIcon,
  SlidersHorizontalIcon,
} from "@lucide/vue";
import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
import { Alert, AlertDescription } from "@/components/ui/alert";
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
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuItem,
  SidebarTrigger,
  useSidebar,
} from "@/components/ui/sidebar";
import SessionsList from "@/components/sidebar/SessionsList.vue";
import WorkspaceRow from "@/components/sidebar/WorkspaceRow.vue";
import { useAppearance } from "@/lib/appearance";
import { themedBrand } from "@/lib/brand";
import {
  activeSession,
  liveIds,
  newSession,
  refreshLiveSessions,
  selectSession,
  sessionGeneration,
} from "@/lib/session-store";
import {
  addWorkspace,
  commandErrorMessage,
  getSettings,
  listModels,
  listSessions,
  listWorkspaces,
  removeWorkspace,
  saveSettings,
  setWorkspacePinned,
  touchWorkspace,
} from "../contracts/commands";
import type {
  AppSettings,
  ModelInfo,
  SessionSummary,
  WorkspaceInfo,
} from "../contracts/types";

const props = defineProps<{ activeWorkspaceId: number | null }>();
const emit = defineEmits<{ select: [workspace: WorkspaceInfo | null] }>();

const THEMES = ["dark", "light", "system"] as const;

const workspaces = ref<WorkspaceInfo[]>([]);
const workspacesLoading = ref(false);
const search = ref("");
/** S3: the magnifier toggles the search field instead of it always taking room. */
const searchOpen = ref(false);
/**
 * S3: the sliders button toggles edit mode. This unit only stores the flag and
 * reflects it on the rows (their actions stay visible); the rename/delete
 * dialogs land in the next unit.
 */
const editMode = ref(false);
// The space whose pin request is in flight, so its button cannot double-fire.
const pinPendingId = ref<number | null>(null);
const adding = ref(false);
/** A `start_session` call is in flight, so the button cannot spawn two pis. */
const creating = ref(false);
const workspaceError = ref<string | null>(null);

/**
 * L4: how often the drawer re-asks which sessions have a live process. The
 * stream already updates the list on every lifecycle event, so this is only a
 * slow safety net for a runtime that never reported one.
 */
const LIVE_POLL_MS = 5000;
let livePollHandle: number | null = null;

/** S5: ids of the workspaces whose session list is open. */
const expandedIds = ref<number[]>([]);
/** S5: the sessions of each workspace, keyed by workspace id. */
const sessionsByWorkspace = ref<Record<number, SessionSummary[]>>({});
/** S5: workspaces whose session fetch is in flight. */
const sessionsLoading = ref<number[]>([]);
const sessionsErrors = ref<Record<number, string>>({});

// The brand mark is theme-aware: the white-on-dark variant on the dark palette
// and its black-on-light sibling otherwise, so a theme switch switches the logo
// too. `logoFailed` keeps the ∇ fallback while an asset is missing (S7).
const logoFailed = ref(false);
const { isDark, setMode } = useAppearance();
// Referenced as a runtime URL (not a static asset import) because Vite fails the
// build for a missing asset, and the file is expected to be missing at first.
const logoSource = computed(() => themedBrand("mark", isDark.value));
// Wordmark only (no mark): the header composes mark + wordmark side by side, so
// it must never show the combined asset (that would duplicate the mark).
const wordmarkFailed = ref(false);
const wordmarkSource = computed(() => themedBrand("wordmark", isDark.value));
// `SidebarTrigger` renders a fixed icon, so the collapsed state needs the raw
// toggle: there the app mark *is* the expand control.
const { toggleSidebar } = useSidebar();

const settingsOpen = ref(false);
// S6 (next unit): the ⋯ menu's edit item opens this read-only placeholder.
const editOpen = ref(false);
const editingWorkspace = ref<WorkspaceInfo | null>(null);
// Mirrors `Settings::default()` in `src-tauri/src/settings.rs` for the moment
// before `get_settings` resolves; it is one fallback model id, never the model
// list (that always comes from `list_models`, S7).
const settings = ref<AppSettings>({
  theme: "dark",
  piBinary: "pi",
  provider: "nan",
  model: "deepseek-v4-flash",
});
// S7: the model options come from `list_models`, never from a list kept here.
const models = ref<ModelInfo[]>([]);
const savingSettings = ref(false);
const settingsError = ref<string | null>(null);

/**
 * `add_workspace` rejects with a plain Display string. The one rejection worth
 * translating is the store's UNIQUE constraint on `path`: the raw SQLite text
 * ("store sqlite error: UNIQUE constraint failed: workspaces.path") tells a user
 * nothing about what to do next.
 */
function addErrorMessage(cause: unknown): string {
  const message = commandErrorMessage(cause);
  if (message.includes("UNIQUE constraint failed")) {
    return "That workspace path is already in the list.";
  }
  return message;
}

/**
 * S3/S4: the search text narrows the loaded list by name or by path. The order
 * stays the server's (pinned first, then most recently used).
 */
const visibleWorkspaces = computed<WorkspaceInfo[]>(() => {
  const query = search.value.trim().toLowerCase();
  if (query === "") return workspaces.value;
  return workspaces.value.filter(
    (workspace) =>
      workspace.name.toLowerCase().includes(query) ||
      workspace.path.toLowerCase().includes(query),
  );
});

async function refreshWorkspaces() {
  workspacesLoading.value = true;
  try {
    workspaces.value = await listWorkspaces();
    workspaceError.value = null;
    // Keep the selection valid and current: drop it if the active row
    // disappeared (otherwise the chat would hold a deleted workspace), and
    // re-emit the refreshed row so pinned/lastUsedAt changes reach App.vue.
    const active = workspaces.value.find((w) => w.id === props.activeWorkspaceId);
    emit("select", active ?? workspaces.value[0] ?? null);
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  } finally {
    workspacesLoading.value = false;
  }
}

/**
 * S8: selecting is also a usage signal. The selection lands immediately, then
 * `touch_workspace` records it and the refresh re-sorts the list by recency.
 */
async function handleSelectWorkspace(workspace: WorkspaceInfo) {
  emit("select", workspace);
  try {
    await touchWorkspace(workspace.id);
    await refreshWorkspaces();
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  }
}

/**
 * S5: a row click selects the workspace (the existing path) and toggles its
 * session list. Clicking the already-selected row only toggles: re-touching it
 * would rewrite recency for an action that changed nothing.
 */
async function handleActivateWorkspace(workspace: WorkspaceInfo) {
  toggleExpanded(workspace.id);
  if (workspace.id !== props.activeWorkspaceId) {
    await handleSelectWorkspace(workspace);
  }
}

function isExpanded(workspaceId: number): boolean {
  return expandedIds.value.includes(workspaceId);
}

function toggleExpanded(workspaceId: number) {
  expandedIds.value = isExpanded(workspaceId)
    ? expandedIds.value.filter((id) => id !== workspaceId)
    : [...expandedIds.value, workspaceId];
}

/** S5: the cached sessions of a workspace, empty until its first fetch lands. */
function sessionsFor(workspaceId: number): SessionSummary[] {
  return sessionsByWorkspace.value[workspaceId] ?? [];
}

function sessionsLoadingFor(workspaceId: number): boolean {
  return sessionsLoading.value.includes(workspaceId);
}

/**
 * S5: reads one workspace's session index. `list_sessions` is the only source;
 * the cache exists so the list keeps rendering while a refresh is in flight.
 */
async function loadSessions(workspaceId: number) {
  if (sessionsLoading.value.includes(workspaceId)) return;
  sessionsLoading.value = [...sessionsLoading.value, workspaceId];
  try {
    const list = await listSessions(workspaceId);
    sessionsByWorkspace.value = { ...sessionsByWorkspace.value, [workspaceId]: list };
    delete sessionsErrors.value[workspaceId];
  } catch (cause) {
    sessionsErrors.value = {
      ...sessionsErrors.value,
      [workspaceId]: commandErrorMessage(cause),
    };
  } finally {
    sessionsLoading.value = sessionsLoading.value.filter((id) => id !== workspaceId);
  }
}

/**
 * S5: fetches every expanded workspace. Expanding and re-expanding both come
 * through here, which is why a re-expand refreshes instead of trusting the
 * cache: a session may have started while the row was closed.
 */
function syncSessions(ids: number[]) {
  for (const id of ids) void loadSessions(id);
  // Collapsed rows drop their cache so a later expand cannot show a stale list.
  const keep = new Set(ids);
  sessionsByWorkspace.value = Object.fromEntries(
    Object.entries(sessionsByWorkspace.value).filter(([key]) => keep.has(Number(key))),
  );
}

watch(expandedIds, syncSessions);

// N1: a new session (from the button or from the composer's first send) changes
// what each open list must show.
watch(sessionGeneration, () => {
  sessionsByWorkspace.value = {};
  syncSessions(expandedIds.value);
});

/** S2: creates a session for the selected workspace — the one creation path. */
async function handleNewSession() {
  const workspace = workspaces.value.find((w) => w.id === props.activeWorkspaceId);
  if (workspace === undefined || creating.value) return;
  creating.value = true;
  workspaceError.value = null;
  try {
    // Same usage signal the selection path records; the list itself is not
    // re-sorted under the pointer here, only on the next select or add.
    await touchWorkspace(workspace.id);
    await newSession(workspace.id);
    // S5: open the row so the session it just created is visible.
    if (!isExpanded(workspace.id)) {
      expandedIds.value = [...expandedIds.value, workspace.id];
    }
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  } finally {
    creating.value = false;
  }
}

/**
 * L4: selecting a session in the list makes it the active session and focuses
 * its workspace, so the chat is never showing one workspace while its session
 * belongs to another. The store does the rest — opening the process on demand
 * when it is not live (S3), reusing it when it is (S5) and seeding the
 * transcript from pi's file. Any session that was running before keeps running
 * (S4): this path never stops anything.
 */
async function handleOpenSession(
  workspace: WorkspaceInfo,
  session: SessionSummary,
) {
  if (workspace.id !== props.activeWorkspaceId) {
    await handleSelectWorkspace(workspace);
  }
  await selectSession(session);
}

/** S3: pin/unpin one row; the refreshed server list is the source of truth. */
async function handleTogglePin(workspace: WorkspaceInfo) {
  if (pinPendingId.value !== null) return;
  pinPendingId.value = workspace.id;
  workspaceError.value = null;
  try {
    await setWorkspacePinned(workspace.id, !workspace.pinned);
    await refreshWorkspaces();
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  } finally {
    pinPendingId.value = null;
  }
}

/**
 * S3: the header's folder button. The native folder picker is the whole add
 * flow; cancelling the dialog is not an error, so it silently does nothing. The
 * name is never sent: the store derives it from the path (S6's default).
 */
async function handleBrowseFolder() {
  if (adding.value) return;
  workspaceError.value = null;
  let selected: string | string[] | null;
  try {
    selected = await openFolderDialog({
      directory: true,
      multiple: false,
      title: "Choose a workspace folder",
    });
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
    return;
  }
  const path = Array.isArray(selected) ? selected[0] : selected;
  if (!path) return;
  await addWorkspaceAndSelect(path, null);
}

/** Create the row, refresh, and select it — adding a workspace is an intent. */
async function addWorkspaceAndSelect(path: string, name: string | null) {
  adding.value = true;
  workspaceError.value = null;
  try {
    const created = await addWorkspace(path, name);
    await refreshWorkspaces();
    const added = workspaces.value.find((w) => w.id === created.id) ?? created;
    emit("select", added);
  } catch (cause) {
    workspaceError.value = addErrorMessage(cause);
  } finally {
    adding.value = false;
  }
}

async function handleRemoveWorkspace(workspace: WorkspaceInfo) {
  workspaceError.value = null;
  try {
    await removeWorkspace(workspace.id);
    // The row is gone: drop its cached sessions and its expansion as well.
    expandedIds.value = expandedIds.value.filter((id) => id !== workspace.id);
    delete sessionsByWorkspace.value[workspace.id];
    delete sessionsErrors.value[workspace.id];
    // `refreshWorkspaces` replaces the selection when the active row is gone.
    await refreshWorkspaces();
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  }
}

/**
 * S6: the ⋯ menu's edit item. Rename/alias and the delete confirmation are the
 * next unit's work; until then this opens a read-only dialog that shows exactly
 * what the store holds, so nothing is silently pretended.
 */
function openEditDialog(workspace: WorkspaceInfo) {
  editingWorkspace.value = workspace;
  editOpen.value = true;
}

async function loadModels() {
  try {
    models.value = await listModels();
  } catch (cause) {
    models.value = [];
    settingsError.value = commandErrorMessage(cause);
  }
}

async function loadSettings() {
  try {
    settings.value = await getSettings();
    // S2: the stored mode is applied through the shared appearance module, the
    // only path that applies the palette or toggles the `dark` class (S4).
    setMode(settings.value.theme);
    settingsError.value = null;
  } catch (cause) {
    settingsError.value = commandErrorMessage(cause);
  }
}

async function handleSaveSettings() {
  savingSettings.value = true;
  settingsError.value = null;
  try {
    settings.value = await saveSettings({ ...settings.value });
    // S2: the dialog saves the mode; applying it here makes the change visible
    // as soon as the write lands, without waiting for a reload. Logos follow
    // because they read the same shared state.
    setMode(settings.value.theme);
    settingsOpen.value = false;
  } catch (cause) {
    settingsError.value = commandErrorMessage(cause);
  } finally {
    savingSettings.value = false;
  }
}

// Preferences live in settings.json; read them (and the model options) every
// time the dialog opens so a human edit outside the app is picked up.
watch(settingsOpen, (open) => {
  if (open) {
    void loadSettings();
    void loadModels();
  }
});

onMounted(() => {
  void refreshWorkspaces();
  // Apply the persisted theme once at startup; the dialog re-reads the file on
  // every open so a human edit outside the app is picked up.
  void loadSettings();
  // L4: which sessions have a live process. Events keep this current (the store
  // updates it on every lifecycle frame); the interval only covers the case the
  // drawer needs after a process started or died while it was not listening.
  void refreshLiveSessions();
  livePollHandle = window.setInterval(() => {
    void refreshLiveSessions();
  }, LIVE_POLL_MS);
});

onUnmounted(() => {
  if (livePollHandle === null) return;
  window.clearInterval(livePollHandle);
  livePollHandle = null;
});
</script>

<template>
  <Sidebar collapsible="icon">
    <SidebarHeader>
      <div class="flex items-center gap-2 px-1 py-1">
        <!-- Expanded header, one row: [mark] [wordmark] [collapse control].
             The drawer is still the only place with a collapse control (S8). -->
        <div
          class="flex min-w-0 flex-1 items-center gap-2 group-data-[collapsible=icon]:hidden"
        >
          <!-- Brand mark (S7), no text: the wordmark asset sits next to it. -->
          <img
            v-if="!logoFailed"
            :src="logoSource"
            alt=""
            aria-hidden="true"
            class="size-6 shrink-0 rounded-md object-contain"
            @error="logoFailed = true"
          />
          <!-- Nabla glyph fallback: the del operator, the app's namesake. -->
          <span
            v-else
            aria-hidden="true"
            class="text-lg leading-none font-semibold text-primary select-none"
            >∇</span
          >

          <!-- Wordmark only, centered in the gap between mark and trigger, so the
               row never repeats the mark twice. -->
          <span class="flex min-w-0 flex-1 items-center justify-center">
            <img
              v-if="!wordmarkFailed"
              :src="wordmarkSource"
              alt="Nabla"
              class="h-3 w-auto shrink-0 object-contain"
              @error="wordmarkFailed = true"
            />
            <span v-else class="truncate text-xs font-semibold">Nabla</span>
          </span>

          <SidebarTrigger class="shrink-0" />
        </div>

        <!-- Collapsed: the app mark *is* the expand control, so the rail gains
             no extra chrome beyond the logo. -->
        <Button
          variant="ghost"
          size="icon-sm"
          class="group/expand mx-auto hidden group-data-[collapsible=icon]:flex"
          aria-label="Expand sidebar"
          @click="toggleSidebar"
        >
          <!-- Brand-first at rest; the expand affordance appears on hover and on
               keyboard focus, so the control stays discoverable without the rail
               losing its logo. -->
          <img
            v-if="!logoFailed"
            :src="logoSource"
            alt="Nabla"
            class="size-6 rounded-md object-contain group-hover/expand:hidden group-focus-visible/expand:hidden"
            @error="logoFailed = true"
          />
          <span
            v-else
            aria-hidden="true"
            class="text-lg leading-none font-semibold group-hover/expand:hidden group-focus-visible/expand:hidden"
            >∇</span
          >
          <PanelLeftIcon
            class="hidden group-hover/expand:block group-focus-visible/expand:block"
          />
        </Button>
      </div>

      <!-- S2: New session, full width, just under the header. It creates the
           session for the selected workspace through the shared store, so the
           chat follows it; collapsed, the label gives way to the icon. -->
      <Button
        variant="secondary"
        class="h-9 w-full gap-2 rounded-xl bg-secondary/60 px-3 text-sm font-medium text-secondary-foreground hover:bg-secondary group-data-[collapsible=icon]:size-8 group-data-[collapsible=icon]:self-center group-data-[collapsible=icon]:p-0"
        :disabled="activeWorkspaceId === null || creating"
        aria-label="New session"
        @click="handleNewSession"
      >
        <CirclePlusIcon class="size-4 shrink-0" aria-hidden="true" />
        <span class="group-data-[collapsible=icon]:hidden">New session</span>
      </Button>
    </SidebarHeader>

    <!-- S1: no separator between the header and the content. -->
    <SidebarContent>
      <!-- S3: section title on the left, the three actions on the right. -->
      <div
        class="flex items-center justify-between gap-1 px-3 pt-1 pb-1 group-data-[collapsible=icon]:hidden"
      >
        <span class="text-xs font-medium tracking-wide text-muted-foreground uppercase">
          Workspaces
        </span>
        <div class="flex items-center gap-0.5">
          <!-- Search: toggles the field below instead of it always taking room. -->
          <Button
            variant="ghost"
            size="icon-xs"
            class="text-muted-foreground"
            aria-label="Search workspaces"
            :aria-pressed="searchOpen"
            :aria-expanded="searchOpen"
            @click="searchOpen = !searchOpen"
          >
            <SearchIcon />
          </Button>
          <!-- Edit mode: the flag this unit stores; rows keep their actions
               visible while it is on (the dialogs come next). -->
          <Button
            variant="ghost"
            size="icon-xs"
            :class="editMode ? 'text-foreground' : 'text-muted-foreground'"
            aria-label="Toggle edit mode"
            :aria-pressed="editMode"
            @click="editMode = !editMode"
          >
            <SlidersHorizontalIcon />
          </Button>
          <!-- Add folder: the native picker, the whole add flow (S3). -->
          <Button
            variant="ghost"
            size="icon-xs"
            class="text-muted-foreground"
            aria-label="Add workspace folder"
            :disabled="adding"
            @click="handleBrowseFolder"
          >
            <FolderPlusIcon />
          </Button>
        </div>
      </div>

      <div v-if="searchOpen" class="px-2 pb-1 group-data-[collapsible=icon]:hidden">
        <div class="relative">
          <SearchIcon
            aria-hidden="true"
            class="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground"
          />
          <Input
            v-model="search"
            aria-label="Search workspaces"
            placeholder="Search workspaces…"
            class="h-8 pl-7 text-xs"
          />
        </div>
      </div>

      <!-- S5: the collapsible list. The divider that used to sit here was
           removed: the section header already separates the groups. -->
      <div class="flex min-h-0 flex-col gap-1 px-1 pb-1">
        <p
          v-if="workspaces.length === 0 && !workspacesLoading"
          class="px-2 py-1 text-xs text-muted-foreground group-data-[collapsible=icon]:hidden"
        >
          No workspaces yet — add a folder with the folder button.
        </p>
        <!-- Search came back empty: distinct from "no workspaces". -->
        <p
          v-else-if="!workspacesLoading && visibleWorkspaces.length === 0"
          class="px-2 py-1 text-xs text-muted-foreground group-data-[collapsible=icon]:hidden"
        >
          No workspaces match this search.
        </p>

        <SidebarMenu>
          <SidebarMenuItem v-for="workspace in visibleWorkspaces" :key="workspace.id">
            <WorkspaceRow
              :workspace="workspace"
              :active="workspace.id === activeWorkspaceId"
              :expanded="isExpanded(workspace.id)"
              :edit-mode="editMode"
              :pin-pending="pinPendingId === workspace.id"
              @activate="handleActivateWorkspace(workspace)"
              @toggle-pin="handleTogglePin(workspace)"
              @edit="openEditDialog(workspace)"
              @remove="handleRemoveWorkspace(workspace)"
            />
            <!-- S5: its sessions, lazily fetched and cached per workspace. -->
            <SessionsList
              v-if="isExpanded(workspace.id)"
              :sessions="sessionsFor(workspace.id)"
              :loading="sessionsLoadingFor(workspace.id)"
              :error="sessionsErrors[workspace.id] ?? null"
              :active-session-id="activeSession?.sessionId ?? null"
              :live-session-ids="liveIds"
              @select="handleOpenSession(workspace, $event)"
            />
          </SidebarMenuItem>
        </SidebarMenu>
      </div>

      <Alert v-if="workspaceError" variant="destructive" class="mx-2 mb-2">
        <AlertDescription>{{ workspaceError }}</AlertDescription>
      </Alert>
    </SidebarContent>

    <SidebarFooter>
      <Dialog v-model:open="settingsOpen">
        <Button
          variant="ghost"
          class="w-full justify-start gap-2"
          aria-label="Settings"
          @click="settingsOpen = true"
        >
          <SettingsIcon />
          <span class="group-data-[collapsible=icon]:hidden">Settings</span>
        </Button>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Settings</DialogTitle>
            <DialogDescription>
              Preferences are stored in settings.json.
            </DialogDescription>
          </DialogHeader>

          <div class="grid gap-4">
            <div class="grid gap-2">
              <label for="settings-theme" class="text-sm font-medium">Theme</label>
              <Select v-model="settings.theme">
                <SelectTrigger id="settings-theme" class="w-full">
                  <SelectValue placeholder="Theme" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="theme in THEMES" :key="theme" :value="theme">
                    {{ theme }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div class="grid gap-2">
              <label for="settings-pi-binary" class="text-sm font-medium">pi binary</label>
              <Input id="settings-pi-binary" v-model="settings.piBinary" placeholder="pi" />
            </div>

            <!-- S7: the same model the chat header selects, settable from here
                 too. It is part of this form, so Save persists it with the rest. -->
            <div class="grid gap-2">
              <label for="settings-model" class="text-sm font-medium">Model</label>
              <Select v-model="settings.model">
                <SelectTrigger id="settings-model" class="w-full">
                  <SelectValue placeholder="Model" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="model in models" :key="model.id" :value="model.id">
                    {{ model.name }}
                  </SelectItem>
                </SelectContent>
              </Select>
              <p class="text-xs text-muted-foreground">
                Provider: {{ settings.provider }} · used by the next session spawn.
              </p>
            </div>

            <Alert v-if="settingsError" variant="destructive">
              <AlertDescription>{{ settingsError }}</AlertDescription>
            </Alert>
          </div>

          <DialogFooter>
            <DialogClose as-child>
              <Button variant="outline">Cancel</Button>
            </DialogClose>
            <Button :disabled="savingSettings" @click="handleSaveSettings">
              <CheckIcon data-icon="inline-start" />
              {{ savingSettings ? "Saving…" : "Save" }}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <!-- S6: read-only placeholder behind the row menu's "Edit workspace".
           Alias editing and the delete confirmation are the next unit. -->
      <Dialog v-model:open="editOpen">
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Edit workspace</DialogTitle>
            <DialogDescription>
              Renaming and deleting a workspace land in the next unit. This is
              what the store holds today.
            </DialogDescription>
          </DialogHeader>

          <div class="grid gap-3">
            <div class="grid gap-1">
              <span class="text-sm font-medium">Name</span>
              <p class="text-sm break-words text-muted-foreground">
                {{ editingWorkspace?.name }}
              </p>
            </div>
            <div class="grid gap-1">
              <span class="text-sm font-medium">Path</span>
              <p class="text-sm break-all text-muted-foreground">
                {{ editingWorkspace?.path }}
              </p>
            </div>
          </div>

          <DialogFooter>
            <DialogClose as-child>
              <Button variant="outline">Close</Button>
            </DialogClose>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </SidebarFooter>
  </Sidebar>
</template>
