<script setup lang="ts">
// Left drawer (specs S1, S3, S6, S7-S9): the Nabla logo, the collapse control —
// the only one in the shell (S8) — the workspace list with its add form (native
// folder picker first, manual path second), and the settings dialog in the footer.
// It owns the workspace/settings data flow; the *selection* is lifted to App.vue
// through `select`, which feeds ChatPanel.
import { onMounted, ref, watch } from "vue";
import {
  CheckIcon,
  FolderIcon,
  FolderOpenIcon,
  PlusIcon,
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
  SidebarMenuAction,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarSeparator,
  SidebarTrigger,
} from "@/components/ui/sidebar";
import {
  addWorkspace,
  commandErrorMessage,
  getSettings,
  listWorkspaces,
  removeWorkspace,
  saveSettings,
} from "../contracts/commands";
import type { AppSettings, WorkspaceInfo } from "../contracts/types";

const props = defineProps<{ activeWorkspaceId: number | null }>();
const emit = defineEmits<{ select: [workspace: WorkspaceInfo | null] }>();

const THEMES = ["dark", "light", "system"] as const;

const workspaces = ref<WorkspaceInfo[]>([]);
const workspacesLoading = ref(false);
const newPath = ref("");
const newName = ref("");
const adding = ref(false);
const workspaceError = ref<string | null>(null);
// The brand mark is optional: `public/brand/nabla-logo.png` is dropped in by the
// maintainer, so until it exists the header falls back to the ∇ glyph (S7).
const logoFailed = ref(false);
// Referenced as a runtime URL (not a static asset import) because Vite fails the
// build for a missing asset, and the file is expected to be missing at first.
const logoSource = "/brand/nabla-logo.png";

const settingsOpen = ref(false);
const settings = ref<AppSettings>({ theme: "dark", piBinary: "pi" });
const savingSettings = ref(false);
const settingsError = ref<string | null>(null);

/**
 * The theme preference is applied to `<html>`: `.dark` is the default shipped by
 * index.html, `system` follows the OS and otherwise the light palette is used.
 */
function applyTheme(theme: string) {
  const root = document.documentElement;
  const dark =
    theme === "dark" ||
    (theme !== "light" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  root.classList.toggle("dark", dark);
}

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

async function refreshWorkspaces() {
  workspacesLoading.value = true;
  try {
    workspaces.value = await listWorkspaces();
    workspaceError.value = null;
    // Keep the selection valid: drop it if the active row disappeared, and
    // default to the first workspace so the chat is usable right away.
    const stillThere = workspaces.value.some((w) => w.id === props.activeWorkspaceId);
    if (!stillThere) {
      emit("select", workspaces.value[0] ?? null);
    }
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  } finally {
    workspacesLoading.value = false;
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
 * Primary add flow (S9): the native folder picker. Cancelling the dialog is not
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
    if (workspace.id === props.activeWorkspaceId) {
      emit("select", null);
    }
    await refreshWorkspaces();
  } catch (cause) {
    workspaceError.value = commandErrorMessage(cause);
  }
}

async function loadSettings() {
  try {
    settings.value = await getSettings();
    applyTheme(settings.value.theme);
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
    applyTheme(settings.value.theme);
    settingsOpen.value = false;
  } catch (cause) {
    settingsError.value = commandErrorMessage(cause);
  } finally {
    savingSettings.value = false;
  }
}

// Preferences live in settings.json; read them every time the dialog opens so a
// human edit outside the app is picked up.
watch(settingsOpen, (open) => {
  if (open) void loadSettings();
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
        <!-- The only collapse control in the shell (S8): the central header has
             none, so the drawer alone owns its own visibility. -->
        <SidebarTrigger class="shrink-0" />
        <div class="flex min-w-0 items-center gap-2 group-data-[collapsible=icon]:hidden">
          <!-- Brand mark (S7). `aria-hidden`: the wordmark next to it already
               names the app. The @error handler is what makes the header
               correct both before and after the asset lands. -->
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
          <span class="truncate text-sm font-semibold">Nabla</span>
        </div>
      </div>
    </SidebarHeader>

    <SidebarSeparator />

    <SidebarContent>
      <SidebarGroup>
        <SidebarGroupLabel>Workspaces</SidebarGroupLabel>
        <SidebarGroupContent>
          <p
            v-if="workspaces.length === 0 && !workspacesLoading"
            class="px-2 pb-2 text-xs text-muted-foreground group-data-[collapsible=icon]:hidden"
          >
            No workspaces yet — add a folder path below.
          </p>

          <SidebarMenu>
            <SidebarMenuItem v-for="workspace in workspaces" :key="workspace.id">
              <SidebarMenuButton
                :is-active="workspace.id === activeWorkspaceId"
                :tooltip="workspace.name"
                @click="emit('select', workspace)"
              >
                <FolderIcon />
                <span class="truncate">{{ workspace.name }}</span>
              </SidebarMenuButton>
              <SidebarMenuAction
                show-on-hover
                :aria-label="`Remove ${workspace.name}`"
                @click="handleRemoveWorkspace(workspace)"
              >
                <Trash2Icon />
              </SidebarMenuAction>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarGroupContent>
      </SidebarGroup>

      <!-- Add folder area. Hidden while the drawer is collapsed to icons: a path
           typed into a 3rem-wide drawer is not a flow worth supporting. -->
      <div class="flex flex-col gap-2 px-2 pb-2 group-data-[collapsible=icon]:hidden">
        <Separator />
        <!-- Primary flow (S9): the native folder picker. -->
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
