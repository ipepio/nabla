<script setup lang="ts">
// Left drawer (specs S1-S4, S6-S9): the Nabla logo, the collapse control — the
// only one in the shell (S8) — the "spaces" list (the current workspaces, S1)
// with its filter chips (S2), its search box (S3) and the add flow (S4: native
// folder picker first, manual path second), and the settings dialog in the
// footer. It owns the workspace/settings data flow; the *selection* is lifted to
// App.vue through `select`, which feeds ChatPanel.
import { computed, onMounted, ref, watch } from "vue";
import {
  CheckIcon,
  FolderIcon,
  FolderOpenIcon,
  PinIcon,
  PlusIcon,
  SearchIcon,
  SettingsIcon,
  Trash2Icon,
} from "@lucide/vue";
import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
import {
  Alert,
  AlertDescription,
} from "@/components/ui/alert";
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
import { Separator } from "@/components/ui/separator";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarSeparator,
  SidebarTrigger,
  useSidebar,
} from "@/components/ui/sidebar";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useAppearance } from "@/lib/appearance";
import { themedBrand } from "@/lib/brand";
import {
  addWorkspace,
  commandErrorMessage,
  getSettings,
  listModels,
  listWorkspaces,
  removeWorkspace,
  saveSettings,
  setWorkspacePinned,
  touchWorkspace,
} from "../contracts/commands";
import type { AppSettings, ModelInfo, WorkspaceInfo } from "../contracts/types";

const props = defineProps<{ activeWorkspaceId: number | null }>();
const emit = defineEmits<{ select: [workspace: WorkspaceInfo | null] }>();

const THEMES = ["dark", "light", "system"] as const;

/** S2: the three filters the user asked for. */
type FilterValue = "all" | "recent" | "pinned";

const FILTERS: { value: FilterValue; label: string }[] = [
  { value: "all", label: "All" },
  { value: "recent", label: "Recent" },
  { value: "pinned", label: "Pinned" },
];

const workspaces = ref<WorkspaceInfo[]>([]);
const workspacesLoading = ref(false);
const filter = ref<FilterValue>("all");
const search = ref("");
// The space whose pin request is in flight, so its button cannot double-fire.
const pinPendingId = ref<number | null>(null);
const newPath = ref("");
const newName = ref("");
const adding = ref(false);
const workspaceError = ref<string | null>(null);
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
 * Recency key for the "Recent" filter (S2/S8). A space that was never touched
 * falls back to its creation time, so it is ordered instead of pushed to the
 * edge; ties keep the server order.
 */
function recencyKey(workspace: WorkspaceInfo): string {
  return workspace.lastUsedAt ?? workspace.createdAt;
}

/**
 * S2/S3: chips and search are one client-side pass over the loaded list. "All"
 * keeps the server order (pinned first, then most recently used), "Recent"
 * re-sorts by the recency key and "Pinned" keeps the pinned rows only; the
 * search text then narrows whatever survived, by name or by path.
 */
const visibleWorkspaces = computed<WorkspaceInfo[]>(() => {
  const query = search.value.trim().toLowerCase();
  let list = workspaces.value;
  if (filter.value === "pinned") {
    list = list.filter((workspace) => workspace.pinned);
  }
  if (query !== "") {
    list = list.filter(
      (workspace) =>
        workspace.name.toLowerCase().includes(query) ||
        workspace.path.toLowerCase().includes(query),
    );
  }
  if (filter.value === "recent") {
    list = [...list].sort((a, b) => recencyKey(b).localeCompare(recencyKey(a)));
  }
  return list;
});

/** S2: the chips are controlled, so an empty selection is never rendered. */
function setFilter(value: unknown) {
  if (value === "all" || value === "recent" || value === "pinned") {
    filter.value = value;
  }
}

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
 * `touch_workspace` records it and the refresh re-sorts the list under
 * "Recent".
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

/** S2: pin/unpin one row; the refreshed server list is the source of truth. */
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

async function handleAddWorkspace() {
  const path = newPath.value.trim();
  if (path === "") {
    workspaceError.value = "Workspace path is required.";
    return;
  }
  await addWorkspaceAndSelect(path, newName.value.trim() === "" ? null : newName.value.trim());
}

/**
 * Primary add flow (S4): the native folder picker. Cancelling the dialog is not
 * an error, so it silently leaves the form untouched. The name is never sent:
 * the store derives it from the path, exactly like the manual flow's empty name.
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

/** Shared by both add flows: create the row, refresh, and select it. */
async function addWorkspaceAndSelect(path: string, name: string | null) {
  adding.value = true;
  workspaceError.value = null;
  try {
    const created = await addWorkspace(path, name);
    newPath.value = "";
    newName.value = "";
    await refreshWorkspaces();
    // Adding a workspace is a selection intent: open it.
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
    // `refreshWorkspaces` replaces the selection when the active row is gone.
    await refreshWorkspaces();
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  }
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

          <!-- Wordmark only, so the row never repeats the mark twice. -->
          <img
            v-if="!wordmarkFailed"
            :src="wordmarkSource"
            alt="Nabla"
            class="h-6 w-auto shrink-0 object-contain"
            @error="wordmarkFailed = true"
          />
          <span v-else class="truncate text-sm font-semibold">Nabla</span>

          <SidebarTrigger class="ms-auto shrink-0" />
        </div>

        <!-- Collapsed: the app mark *is* the expand control, so the rail gains
             no extra chrome beyond the logo. -->
        <Button
          variant="ghost"
          size="icon-sm"
          class="mx-auto hidden group-data-[collapsible=icon]:flex"
          aria-label="Expand sidebar"
          @click="toggleSidebar"
        >
          <img
            v-if="!logoFailed"
            :src="logoSource"
            alt="Nabla"
            class="size-6 rounded-md object-contain"
            @error="logoFailed = true"
          />
          <span v-else aria-hidden="true" class="text-lg leading-none font-semibold"
            >∇</span
          >
        </Button>
      </div>
    </SidebarHeader>

    <SidebarSeparator />

    <SidebarContent>
      <SidebarGroup>
        <SidebarGroupLabel>Workspaces</SidebarGroupLabel>
        <SidebarGroupContent>
          <!-- S3: the search box. Filters the loaded list by name or by path. -->
          <div class="px-2 pb-2 group-data-[collapsible=icon]:hidden">
            <div class="relative">
              <SearchIcon
                aria-hidden="true"
                class="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground"
              />
              <Input
                v-model="search"
                aria-label="Search spaces"
                placeholder="Search spaces…"
                class="h-8 pl-7 text-xs"
              />
            </div>
          </div>

          <!-- S2: Todos / Recientes / Pinned. A single-select segmented control;
               the selection is controlled from `filter` and clicking a chip can
               never leave the group without a visible value. -->
          <ToggleGroup
            type="single"
            size="sm"
            variant="outline"
            :model-value="filter"
            class="mx-2 mb-2 w-auto group-data-[collapsible=icon]:hidden"
            @update:model-value="setFilter"
          >
            <ToggleGroupItem
              v-for="option in FILTERS"
              :key="option.value"
              :value="option.value"
              class="flex-1 px-2 text-xs"
            >
              {{ option.label }}
            </ToggleGroupItem>
          </ToggleGroup>

          <p
            v-if="workspaces.length === 0 && !workspacesLoading"
            class="px-2 pb-2 text-xs text-muted-foreground group-data-[collapsible=icon]:hidden"
          >
            No workspaces yet — add a folder path below.
          </p>
          <!-- Search/filter came back empty: distinct from "no workspaces". -->
          <p
            v-else-if="!workspacesLoading && visibleWorkspaces.length === 0"
            class="px-2 pb-2 text-xs text-muted-foreground group-data-[collapsible=icon]:hidden"
          >
            No spaces match this filter.
          </p>

          <SidebarMenu>
            <SidebarMenuItem v-for="workspace in visibleWorkspaces" :key="workspace.id">
              <SidebarMenuButton
                :is-active="workspace.id === activeWorkspaceId"
                :tooltip="workspace.name"
                class="h-auto items-start py-1.5 pr-16 group-data-[collapsible=icon]:pr-2"
                @click="handleSelectWorkspace(workspace)"
              >
                <FolderIcon class="mt-0.5" />
                <span class="flex min-w-0 flex-col">
                  <span class="truncate text-sm leading-5">{{ workspace.name }}</span>
                  <span class="truncate text-xs leading-4 text-muted-foreground">
                    {{ workspace.path }}
                  </span>
                </span>
              </SidebarMenuButton>

              <!-- Row actions sit over the row, never inside the select button. -->
              <div
                class="absolute top-1 right-1 flex items-center gap-0.5 group-data-[collapsible=icon]:hidden"
              >
                <Button
                  variant="ghost"
                  size="icon-xs"
                  :aria-label="workspace.pinned ? `Unpin ${workspace.name}` : `Pin ${workspace.name}`"
                  :aria-pressed="workspace.pinned"
                  :disabled="pinPendingId === workspace.id"
                  :class="[
                    'text-muted-foreground',
                    workspace.pinned
                      ? 'text-primary'
                      : 'opacity-0 group-focus-within/menu-item:opacity-100 group-hover/menu-item:opacity-100 focus-visible:opacity-100',
                  ]"
                  @click.stop="handleTogglePin(workspace)"
                >
                  <PinIcon :class="workspace.pinned ? 'fill-current' : ''" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  class="text-muted-foreground opacity-0 group-focus-within/menu-item:opacity-100 group-hover/menu-item:opacity-100 focus-visible:opacity-100"
                  :aria-label="`Remove ${workspace.name}`"
                  @click.stop="handleRemoveWorkspace(workspace)"
                >
                  <Trash2Icon />
                </Button>
              </div>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarGroupContent>
      </SidebarGroup>

      <!-- Add folder area. Hidden while the drawer is collapsed to icons: a path
           typed into a 3rem-wide drawer is not a flow worth supporting. -->
      <div class="flex flex-col gap-2 px-2 pb-2 group-data-[collapsible=icon]:hidden">
        <Separator />
        <!-- Primary flow (S4): the native folder picker. -->
        <Button
          variant="outline"
          size="sm"
          class="w-full justify-start"
          aria-label="Browse for folder"
          :disabled="adding"
          @click="handleBrowseFolder"
        >
          <FolderOpenIcon data-icon="inline-start" />
          Browse for folder
        </Button>
        <!-- Secondary flow: the path typed by hand, for folders the picker cannot
             reach (remote paths, hidden directories). -->
        <p class="text-xs text-muted-foreground">Or type a path manually:</p>
        <Input
          v-model="newPath"
          aria-label="Workspace path"
          placeholder="/Users/me/projects/app"
          @keyup.enter="handleAddWorkspace"
        />
        <Input
          v-model="newName"
          aria-label="Workspace name (optional)"
          placeholder="Name (optional)"
          @keyup.enter="handleAddWorkspace"
        />
        <Button size="sm" :disabled="adding" @click="handleAddWorkspace">
          <PlusIcon data-icon="inline-start" />
          {{ adding ? "Adding…" : "Add workspace" }}
        </Button>
        <p class="text-xs text-muted-foreground">
          The path must be a folder that already exists on this machine.
        </p>

        <Alert v-if="workspaceError" variant="destructive">
          <AlertDescription>{{ workspaceError }}</AlertDescription>
        </Alert>
      </div>
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
    </SidebarFooter>
  </Sidebar>
</template>
