<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

const greeting = ref("");
const greetingName = ref("nabla");

async function greet() {
  greeting.value = await invoke("greet", { name: greetingName.value });
}
</script>

<template>
  <v-app>
    <v-app-bar elevation="0">
      <v-app-bar-title class="font-weight-bold">
        <span class="text-primary">Nabla</span>
        <span class="text-medium-emphasis text-caption ml-3">desktop harness · shell</span>
      </v-app-bar-title>
    </v-app-bar>

    <v-main>
      <v-container max-width="640">
        <!-- Temporary smoke-test panel: proves the Rust bridge end-to-end.
             Replaced by the workspaces panel in T6. -->
        <v-card elevation="2" rounded="lg">
          <v-card-title class="text-subtitle-1">Puente Rust</v-card-title>
          <v-card-text>
            <v-text-field
              v-model="greetingName"
              label="Nombre"
              variant="outlined"
              density="comfortable"
              hide-details
            />
            <v-btn
              class="mt-4"
              color="primary"
              variant="flat"
              prepend-icon="mdi-lightning-bolt"
              @click="greet"
            >
              Invoke greet
            </v-btn>
            <v-alert
              v-if="greeting"
              class="mt-4"
              type="info"
              variant="tonal"
              density="compact"
            >
              {{ greeting }}
            </v-alert>
          </v-card-text>
        </v-card>
      </v-container>
    </v-main>
  </v-app>
</template>
