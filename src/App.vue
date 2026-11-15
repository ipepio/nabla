<script setup lang="ts">
import { onMounted, ref } from "vue";
// Imported locally because main.ts installs the Vuetify plugin without a
// component registry; local registration keeps this view self-contained.
import {
  VAlert,
  VApp,
  VAppBar,
  VAppBarTitle,
  VBtn,
  VCard,
  VCardText,
  VCardTitle,
  VCol,
  VContainer,
  VList,
  VListItem,
  VMain,
  VProgressLinear,
  VRow,
  VSelect,
  VSnackbar,
  VTextField,
} from "vuetify/components";
import {
  addWorkspace,
  commandErrorMessage,
  getSettings,
  listWorkspaces,
  removeWorkspace,
  saveSettings,
} from "./contracts/commands";
import type { AppSettings, WorkspaceInfo } from "./contracts/types";
// The chat tracer (R4) is a view of its own; App.vue only supplies workspaces.
import ChatPanel from "./components/ChatPanel.vue";

const THEMES: string[] = ["dark", "light", "system"];

const workspaces = ref<WorkspaceInfo[]>([]);
const workspacesLoading = ref(false);
const newPath = ref("");
const newName = ref("");
const adding = ref(false);

const settings = ref<AppSettings>({ theme: "dark", piBinary: "pi" });
const savingSettings = ref(false);

const error = ref<string | null>(null);
const snackbar = ref(false);
const snackbarText = ref("");

function notify(message: string) {
  snackbarText.value = message;
  snackbar.value = true;
}

async function refreshWorkspaces() {
  workspacesLoading.value = true;
  try {
    workspaces.value = await listWorkspaces();
    error.value = null;
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    workspacesLoading.value = false;
  }
}

async function handleAddWorkspace() {
  const path = newPath.value.trim();
  if (path === "") {
    error.value = "Workspace path is required.";
    return;
  }
  const name = newName.value.trim();
  adding.value = true;
  try {
    await addWorkspace(path, name === "" ? null : name);
    newPath.value = "";
    newName.value = "";
    await refreshWorkspaces();
    notify("Workspace added.");
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    adding.value = false;
  }
}

async function handleRemoveWorkspace(id: number) {
  try {
    await removeWorkspace(id);
    await refreshWorkspaces();
    notify("Workspace removed.");
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  }
}

async function loadSettings() {
  try {
    settings.value = await getSettings();
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  }
}

async function handleSaveSettings() {
  savingSettings.value = true;
  try {
    settings.value = await saveSettings({ ...settings.value });
    error.value = null;
    notify("Settings saved.");
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    savingSettings.value = false;
  }
}

onMounted(() => {
  void refreshWorkspaces();
  void loadSettings();
});
</script>

<template>
  <v-app>
    <v-app-bar elevation="0">
      <v-app-bar-title class="font-weight-bold">
        <span class="text-primary">Nabla</span>
        <span class="text-medium-emphasis text-caption ml-3">
          desktop harness · shell
        </span>
      </v-app-bar-title>
    </v-app-bar>

    <v-main>
      <v-container max-width="900">
        <v-alert
          v-if="error"
          class="mb-4"
          type="error"
          variant="tonal"
          closable
          @click:close="error = null"
        >
          {{ error }}
        </v-alert>

        <!-- Workspaces: the app-owned store of folders (spec S3 metadata only). -->
        <v-card class="mb-6" elevation="2" rounded="lg">
          <v-card-title class="text-subtitle-1">Workspaces</v-card-title>
          <v-card-text>
            <v-progress-linear
              v-if="workspacesLoading"
              class="mb-4"
              indeterminate
              rounded
            />

            <p v-if="workspaces.length === 0 && !workspacesLoading" class="text-medium-emphasis">
              No workspaces yet — add a folder path below.
            </p>

            <v-list v-else density="comfortable">
              <v-list-item
                v-for="workspace in workspaces"
                :key="workspace.id"
                :title="workspace.name"
                :subtitle="`${workspace.path} · ${workspace.createdAt}`"
              >
                <template #append>
                  <v-btn
                    icon="mdi-delete"
                    variant="text"
                    color="error"
                    size="small"
                    :aria-label="`Remove ${workspace.name}`"
                    @click="handleRemoveWorkspace(workspace.id)"
                  />
                </template>
              </v-list-item>
            </v-list>

            <v-row class="mt-2" density="comfortable">
              <v-col cols="12" md="6">
                <v-text-field
                  v-model="newPath"
                  label="Workspace path"
                  placeholder="/Users/me/projects/app"
                  variant="outlined"
                  density="comfortable"
                  hide-details
                />
              </v-col>
              <v-col cols="12" md="4">
                <v-text-field
                  v-model="newName"
                  label="Name (optional)"
                  variant="outlined"
                  density="comfortable"
                  hide-details
                />
              </v-col>
              <v-col cols="12" md="2">
                <v-btn
                  block
                  color="primary"
                  :loading="adding"
                  @click="handleAddWorkspace"
                >
                  Add workspace
                </v-btn>
              </v-col>
            </v-row>
          </v-card-text>
        </v-card>

        <!-- Settings: user-editable preferences from settings.json (spec S2). -->
        <v-card class="mb-6" elevation="2" rounded="lg">
          <v-card-title class="text-subtitle-1">Settings</v-card-title>
          <v-card-text>
            <v-row density="comfortable">
              <v-col cols="12" md="4">
                <v-select
                  v-model="settings.theme"
                  :items="THEMES"
                  label="Theme"
                  variant="outlined"
                  density="comfortable"
                  hide-details
                />
              </v-col>
              <v-col cols="12" md="8">
                <v-text-field
                  v-model="settings.piBinary"
                  label="pi binary"
                  variant="outlined"
                  density="comfortable"
                  hide-details
                />
              </v-col>
            </v-row>
            <v-btn
              class="mt-4"
              color="primary"
              variant="flat"
              :loading="savingSettings"
              @click="handleSaveSettings"
            >
              Save
            </v-btn>
          </v-card-text>
        </v-card>

        <!-- Chat: the pi runtime tracer (R4) — streams one session's output. -->
        <ChatPanel class="mt-6" :workspaces="workspaces" />
      </v-container>
    </v-main>

    <v-snackbar v-model="snackbar" :timeout="2500" color="success">
      {{ snackbarText }}
    </v-snackbar>
  </v-app>
</template>
