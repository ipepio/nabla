<script setup lang="ts">
// Permission mode chip (S1/S2/S5 of `odd/tasks/permission-modes.md`): the mode
// the agent is in, in the chat header next to the theme switch. Clicking cycles
// read → write → yolo.
//
// The chip keeps no opinion of its own. Backend model:
//
//   - on mount it reads the persisted mode, so a relaunch shows what
//     `settings.json` holds instead of a fresh guess;
//   - every click goes through the contracts seam, which persists the mode and
//     rewrites `<data_dir>/permissions.json` — the projection the loaded pi
//     extension reads on EVERY tool call. That file, not this component, is what
//     makes the change reach a live session (S2: no restart, no new spawn).
//
// No `watch`: the value only changes through this control and the mount read,
// and a watcher's getter would run during setup — the TDZ trap this codebase has
// already paid for twice.
import { computed, onMounted, ref } from "vue";
import { Button } from "@/components/ui/button";
import { getPermissionMode, setPermissionMode } from "@/contracts/commands";
import type { PermissionMode } from "@/contracts/types";
import { t, type MessageKey } from "@/lib/i18n";

/** S1: the order one click per mode follows. */
const CYCLE: readonly PermissionMode[] = ["read", "write", "yolo"];

/**
 * S6: `write` until the file answers. It is the fail-safe default of the
 * preference and what the gate itself falls back to, so the chip and the agent
 * agree even when nothing could be read.
 */
const mode = ref<PermissionMode>("write");

/** A click in flight: the button is disabled so a double click cannot skip a mode. */
const saving = ref(false);

/** Keys rather than template strings: `vue-tsc` then checks each one exists. */
const LABEL_KEYS: Record<PermissionMode, MessageKey> = {
  read: "chat.permissionMode.read",
  write: "chat.permissionMode.write",
  yolo: "chat.permissionMode.yolo",
};

/** What each mode actually does — no euphemisms, especially for `yolo` (S5). */
const HINT_KEYS: Record<PermissionMode, MessageKey> = {
  read: "chat.permissionMode.readHint",
  write: "chat.permissionMode.writeHint",
  yolo: "chat.permissionMode.yoloHint",
};

const label = computed(() => t(LABEL_KEYS[mode.value]));
const hint = computed(() => t(HINT_KEYS[mode.value]));

/** The mode one click asks for, in cycle order. */
const next = computed<PermissionMode>(() => {
  const at = CYCLE.indexOf(mode.value);
  return CYCLE[(at + 1) % CYCLE.length] ?? "write";
});

const switchLabel = computed(() =>
  t("chat.permissionMode.switch", { mode: t(LABEL_KEYS[next.value]) }),
);

async function cycle(): Promise<void> {
  if (saving.value) return;
  saving.value = true;
  try {
    // The answer is what the file now holds: rendering it (instead of the value
    // this component asked for) keeps the chip honest if Rust normalized it.
    mode.value = await setPermissionMode(next.value);
  } catch {
    // A write that did not land leaves the chip on the mode it already showed.
    // Painting the new one would claim the agent obeys something it does not.
  } finally {
    saving.value = false;
  }
}

onMounted(async () => {
  try {
    mode.value = await getPermissionMode();
  } catch {
    // Keep the default: the gate reads an unreadable projection as `write` too.
  }
});
</script>

<template>
  <!-- Small on purpose: this is a status control, not a settings page. -->
  <Button
    variant="outline"
    size="xs"
    :class="mode === 'yolo' ? 'text-destructive' : 'text-muted-foreground'"
    :title="hint"
    :aria-label="switchLabel"
    :aria-busy="saving"
    :disabled="saving"
    data-slot="permission-mode"
    @click="cycle"
  >
    {{ label }}
  </Button>
</template>
