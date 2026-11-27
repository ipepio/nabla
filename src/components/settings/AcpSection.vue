<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import {
  CheckCircle2Icon,
  LogInIcon,
  PlusIcon,
  RefreshCwIcon,
  TerminalIcon,
  Trash2Icon,
  XCircleIcon,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  acpDeleteCustomAgent,
  acpListAgents,
  acpSaveCustomAgent,
  acpSetAgentEnabled,
  acpStartLogin,
  closeShell,
  commandErrorMessage,
  subscribeShellOutput,
  writeShell,
} from "@/contracts/commands";
import type { AcpAgentStatus, ShellOutputEvent } from "@/contracts/types";

const emit = defineEmits<{
  (e: "signed-in"): void;
}>();

const agents = ref<AcpAgentStatus[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const loginSessionId = ref<string | null>(null);
const loginAgentName = ref<string>("");
const loginLog = ref<string>("");
const loginInput = ref<string>("");
let unlistenShell: (() => void) | null = null;

// Custom ACP CLI form state
const showAddForm = ref(false);
const savingCustom = ref(false);
const customName = ref("");
const customCommand = ref("");
const customArgs = ref("acp");
const customLoginCommand = ref("");

async function load(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    agents.value = await acpListAgents();
  } catch (err) {
    error.value = commandErrorMessage(err);
  } finally {
    loading.value = false;
  }
}

function splitArgs(raw: string): string[] {
  return raw
    .trim()
    .split(/\s+/)
    .filter((s) => s.length > 0);
}

async function addCustomAgent(): Promise<void> {
  if (!customName.value.trim() || !customCommand.value.trim()) return;
  savingCustom.value = true;
  error.value = null;
  try {
    agents.value = await acpSaveCustomAgent({
      name: customName.value.trim(),
      command: customCommand.value.trim(),
      args: splitArgs(customArgs.value),
      loginCommand: customLoginCommand.value.trim() || null,
      loginArgs: [],
    });
    customName.value = "";
    customCommand.value = "";
    customArgs.value = "acp";
    customLoginCommand.value = "";
    showAddForm.value = false;
    emit("signed-in");
  } catch (err) {
    error.value = commandErrorMessage(err);
  } finally {
    savingCustom.value = false;
  }
}

async function toggleEnabled(agent: AcpAgentStatus): Promise<void> {
  error.value = null;
  try {
    agents.value = await acpSetAgentEnabled(agent.id, !agent.enabled);
    emit("signed-in");
  } catch (err) {
    error.value = commandErrorMessage(err);
  }
}

async function removeCustomAgent(agentId: string): Promise<void> {
  error.value = null;
  try {
    agents.value = await acpDeleteCustomAgent(agentId);
    emit("signed-in");
  } catch (err) {
    error.value = commandErrorMessage(err);
  }
}

function stripAnsi(raw: string): string {
  // eslint-disable-next-line no-control-regex
  return raw.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, "").replace(/\x1b\][^\x07]*\x07/g, "");
}

async function startLogin(agent: AcpAgentStatus): Promise<void> {
  error.value = null;
  await closeLoginSession();
  try {
    if (!unlistenShell) {
      unlistenShell = await subscribeShellOutput((ev: ShellOutputEvent) => {
        if (!loginSessionId.value || ev.sessionId !== loginSessionId.value) return;
        if (ev.data) {
          loginLog.value = (loginLog.value + stripAnsi(ev.data)).slice(-8000);
        }
        if (ev.stream === "exit") {
          loginSessionId.value = null;
          void load();
          emit("signed-in");
        }
      });
    }
    loginAgentName.value = agent.name;
    loginLog.value = "";
    const opened = await acpStartLogin(agent.id);
    loginSessionId.value = opened.sessionId;
  } catch (err) {
    error.value = commandErrorMessage(err);
  }
}

async function sendLoginInput(): Promise<void> {
  if (!loginSessionId.value) return;
  const text = loginInput.value;
  loginInput.value = "";
  try {
    await writeShell(loginSessionId.value, `${text}\r`);
  } catch (err) {
    error.value = commandErrorMessage(err);
  }
}

async function closeLoginSession(): Promise<void> {
  if (!loginSessionId.value) return;
  const sid = loginSessionId.value;
  loginSessionId.value = null;
  try {
    await closeShell(sid);
  } catch {
    // ignore
  }
  await load();
  emit("signed-in");
}

onMounted(() => {
  void load();
});

onBeforeUnmount(() => {
  if (unlistenShell) {
    unlistenShell();
    unlistenShell = null;
  }
  if (loginSessionId.value) {
    void closeShell(loginSessionId.value).catch(() => {});
  }
});
</script>

<template>
  <section class="space-y-3">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <div class="min-w-0">
        <h3 class="text-xs font-semibold text-foreground">
          CLI Connections
        </h3>
        <p class="mt-0.5 text-[11px] text-muted-foreground">
          Auto-detects installed ACP CLIs (Antigravity, Claude Code, Gemini CLI, Codex,
          Cursor Agent, OpenCode, Qwen, Kimi, Goose) or add any custom CLI to switch
          models inside the same Nabla session.
        </p>
      </div>
      <div class="flex items-center gap-1.5">
        <Button
          variant="outline"
          size="sm"
          class="h-7 gap-1 text-xs"
          @click="showAddForm = !showAddForm"
        >
          <PlusIcon class="size-3.5" />
          Add CLI
        </Button>
        <Button
          variant="outline"
          size="sm"
          class="h-7 gap-1.5 text-xs"
          :disabled="loading"
          @click="load"
        >
          <RefreshCwIcon class="size-3.5" :class="loading ? 'animate-spin' : ''" />
          Refresh
        </Button>
      </div>
    </div>

    <form
      v-if="showAddForm"
      class="space-y-2.5 rounded-xl border border-border bg-card p-3"
      @submit.prevent="addCustomAgent"
    >
      <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
        <label class="space-y-1 text-[11px] text-muted-foreground">
          <span>Display name</span>
          <input
            v-model="customName"
            type="text"
            required
            placeholder="e.g. Z.ai Code"
            class="h-7 w-full rounded-md border border-border bg-background px-2 text-xs text-foreground outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
          />
        </label>
        <label class="space-y-1 text-[11px] text-muted-foreground">
          <span>Command (binary or path)</span>
          <input
            v-model="customCommand"
            type="text"
            required
            placeholder="e.g. zai or /usr/local/bin/my-agent"
            class="h-7 w-full rounded-md border border-border bg-background px-2 font-mono text-xs text-foreground outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
          />
        </label>
        <label class="space-y-1 text-[11px] text-muted-foreground">
          <span>ACP arguments</span>
          <input
            v-model="customArgs"
            type="text"
            placeholder="e.g. acp or --acp"
            class="h-7 w-full rounded-md border border-border bg-background px-2 font-mono text-xs text-foreground outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
          />
        </label>
        <label class="space-y-1 text-[11px] text-muted-foreground">
          <span>Login command (optional)</span>
          <input
            v-model="customLoginCommand"
            type="text"
            placeholder="Defaults to main command"
            class="h-7 w-full rounded-md border border-border bg-background px-2 font-mono text-xs text-foreground outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
          />
        </label>
      </div>
      <div class="flex justify-end gap-1.5">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          class="h-7 text-xs"
          @click="showAddForm = false"
        >
          Cancel
        </Button>
        <Button type="submit" size="sm" class="h-7 text-xs" :disabled="savingCustom">
          Save CLI
        </Button>
      </div>
    </form>

    <div
      v-if="error"
      class="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive"
    >
      {{ error }}
    </div>

    <div class="space-y-2">
      <div
        v-for="agent in agents"
        :key="agent.id"
        class="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border bg-secondary/40 px-3 py-2.5"
      >
        <div class="min-w-0 flex-1 space-y-0.5">
          <div class="flex flex-wrap items-center gap-2">
            <span class="text-xs font-medium text-foreground">
              {{ agent.name }}
            </span>
            <span
              v-if="agent.installed && agent.authenticated"
              class="inline-flex items-center gap-1 rounded-full bg-emerald-500/15 px-2 py-0.5 text-[10px] font-medium text-emerald-400"
            >
              <CheckCircle2Icon class="size-3" />
              Ready
            </span>
            <span
              v-else-if="agent.installed"
              class="inline-flex items-center gap-1 rounded-full bg-amber-500/15 px-2 py-0.5 text-[10px] font-medium text-amber-400"
            >
              Sign in needed
            </span>
            <span
              v-else
              class="inline-flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-[10px] font-medium text-muted-foreground"
            >
              <XCircleIcon class="size-3" />
              Not installed
            </span>
          </div>
          <p class="text-[11px] text-muted-foreground">
            {{ agent.installed ? agent.description : agent.hint }}
          </p>
        </div>

        <div class="flex shrink-0 items-center gap-1.5">
          <Button
            v-if="agent.installed"
            :variant="agent.enabled ? 'default' : 'secondary'"
            size="sm"
            class="h-7 text-xs"
            @click="toggleEnabled(agent)"
          >
            {{ agent.enabled ? "Enabled" : "Enable" }}
          </Button>
          <Button
            v-if="agent.installed"
            variant="outline"
            size="sm"
            class="h-7 gap-1.5 text-xs"
            @click="startLogin(agent)"
          >
            <LogInIcon class="size-3.5" />
            {{ agent.authenticated ? "Re-authenticate" : "Sign in" }}
          </Button>
          <Button
            v-if="agent.custom"
            variant="ghost"
            size="icon-xs"
            class="text-muted-foreground hover:text-destructive"
            title="Remove custom CLI"
            @click="removeCustomAgent(agent.id)"
          >
            <Trash2Icon class="size-3.5" />
          </Button>
        </div>
      </div>
    </div>

    <div
      v-if="loginSessionId"
      class="rounded-xl border border-border bg-card p-3 space-y-2"
    >
      <div class="flex items-center justify-between gap-2">
        <div class="flex items-center gap-1.5 text-xs font-medium text-foreground">
          <TerminalIcon class="size-3.5 text-primary" />
          Signing in to {{ loginAgentName }}
        </div>
        <Button variant="ghost" size="sm" class="h-6 text-xs" @click="closeLoginSession">
          Close
        </Button>
      </div>
      <pre
        class="max-h-52 overflow-y-auto whitespace-pre-wrap rounded-md bg-secondary/60 p-2.5 font-mono text-[11px] leading-relaxed text-foreground"
      >{{ loginLog || "Starting CLI login..." }}</pre>
      <form class="flex items-center gap-2" @submit.prevent="sendLoginInput">
        <input
          v-model="loginInput"
          type="text"
          placeholder="Type input or press Enter to send to the CLI..."
          class="h-7 flex-1 rounded-md border border-border bg-background px-2 text-xs text-foreground outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
        />
        <Button type="submit" size="sm" class="h-7 text-xs">Send</Button>
      </form>
    </div>
  </section>
</template>
