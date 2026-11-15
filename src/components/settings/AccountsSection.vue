<script setup lang="ts">
/**
 * Subscription accounts (specs S3, S4, S5 of `odd/tasks/pi-cli-bridge.md`).
 *
 * S5: the parent renders this section only when `VITE_ENABLE_PROVIDER_LOGIN` is
 * on. The flag is read by `SettingsDialog.vue` with the same `readFlag` helper as
 * `VITE_ENABLE_CUSTOM_PROVIDERS` (on by default, only an explicit
 * `0`/`false`/`off` turns it off), because that is where every build-time switch
 * of the dialog lives; this component never declares a second copy of the rule.
 *
 * S3: "Sign in" is the pi subscription login, not an api key. It starts
 * `pi auth login <provider> --no-browser` through the Rust bridge, which is the
 * device-code flow: pi prints a URL and a user code, the human approves it in a
 * browser, and the code pi shows is pasted back here. The child's output is
 * streamed live (`pi-cli` events) and the pasted code goes to its stdin.
 * There is only ever one login at a time: starting another stops the previous
 * one, and leaving this pane stops whatever is live.
 *
 * S4: the badge is `pi auth check` per provider — never a guess — and "Sign out"
 * runs `pi auth logout <provider>` behind a confirmation. Credentials stay in
 * pi's `auth.json`: this component never sees a secret, only a state word.
 *
 * Layout: rows carry no rule of their own (no `border-b`), and this section adds
 * no separator at all — the pane's rules only sit between major blocks.
 */
import {
  computed,
  nextTick,
  onBeforeUnmount,
  onMounted,
  ref,
  watch,
  type ComponentPublicInstance,
} from "vue";
import { Badge, type BadgeVariants } from "@/components/ui/badge";
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
import { useI18n, type MessageKey } from "@/lib/i18n";
import {
  commandErrorMessage,
  piAuthCheck,
  piCliRun,
  piCliStart,
  piCliStop,
  piCliWrite,
  subscribePiCli,
} from "@/contracts/commands";
import type {
  PiAuthState,
  PiAuthStatus,
  PiCliEvent,
  ProviderSummary,
} from "@/contracts/types";

/**
 * S4: the providers pi knows, as the parent already read them from pi's own
 * configuration. Passed in so opening the dialog reads the file once.
 */
const props = defineProps<{ providers: ProviderSummary[] }>();

/** S3/S4: every visible string, both locales (S2 of `i18n-and-settings-polish.md`). */
const { t } = useI18n();

/**
 * S5: the subscription-capable ids that must get a row even when pi's
 * `models.json` does not mention them — a provider nobody configured is exactly
 * the one whose sign-in button has to be reachable.
 */
const WELL_KNOWN_PROVIDERS = [
  "openai",
  "openai-codex",
  "anthropic",
  "google",
] as const;

/**
 * S4: the deadline of the logout call. Rust defaults to 20 s as well; passing it
 * explicitly keeps the intent visible at the call site.
 */
const LOGOUT_TIMEOUT_MS = 20_000;

/** S4: how each state is painted. Ready wins the emphasis; unknown never does. */
const AUTH_STATE_VARIANTS: Record<
  PiAuthState,
  NonNullable<BadgeVariants["variant"]>
> = {
  ready: "default",
  not_ready: "secondary",
  invalid: "destructive",
  unknown: "outline",
};

/** S4: the badge label of each state. */
const AUTH_STATE_KEYS: Record<PiAuthState, MessageKey> = {
  ready: "settings.accounts.state.ready",
  not_ready: "settings.accounts.state.notReady",
  invalid: "settings.accounts.state.invalid",
  unknown: "settings.accounts.state.unknown",
};

/**
 * S5: the rows are the configured providers plus the well-known subscription
 * ids, deduplicated in first-seen order so a provider that is both never shows
 * twice.
 */
const rows = computed<string[]>(() => {
  const ids = [
    ...props.providers.map((entry) => entry.id),
    ...WELL_KNOWN_PROVIDERS,
  ];
  return [...new Set(ids.filter((id) => id.trim() !== ""))];
});

/** S4: the last state pi reported per provider id. Absent means "not asked yet". */
const statuses = ref<Record<string, PiAuthStatus>>({});

/** Inline failures per provider: a rejected call keeps its reason on its row. */
const errors = ref<Record<string, string>>({});

/** The provider whose sign-in is being started, `null` when idle. */
const starting = ref<string | null>(null);
/** The provider whose sign-out is running, `null` when idle. */
const signingOut = ref<string | null>(null);

/** S3: the one live login session, `null` when there is none. */
const session = ref<{ provider: string; sessionId: string } | null>(null);
/** S3: the streamed output of the live login, oldest line first. */
const loginLines = ref<string[]>([]);
/** S3: the code the human pastes back to the child's stdin. */
const loginCode = ref("");
/** A write to the live session's stdin is in flight. */
const sendingCode = ref(false);
/** The child is alive and can still read a code. */
const loginRunning = ref(false);
/** The child left on its own. */
const loginFinished = ref(false);
/** Its exit code, `null` when it was killed by a signal (or still running). */
const loginExitCode = ref<number | null>(null);

/** S3: the output block, scrolled to the newest line as it arrives. */
const outputEl = ref<HTMLElement | null>(null);

/** S4: the confirmation dialog of the sign-out action. */
const signOutOpen = ref(false);
const signOutTarget = ref<string | null>(null);

/** The unsubscribe handle of the `pi-cli` listener, `null` until it resolves. */
let unsubscribeEvents: (() => void) | null = null;
/** Set on unmount, so a listener that resolves late is dropped instead of kept. */
let disposed = false;

function stateOf(provider: string): PiAuthState {
  return statuses.value[provider]?.state ?? "unknown";
}

function setRowError(provider: string, message: string) {
  errors.value = { ...errors.value, [provider]: message };
}

function clearRowError(provider: string) {
  if (!(provider in errors.value)) return;
  const next = { ...errors.value };
  delete next[provider];
  errors.value = next;
}

/** S3: a function ref, because a plain `ref` inside `v-for` collects an array. */
function bindOutput(el: Element | ComponentPublicInstance | null) {
  outputEl.value = el instanceof HTMLElement ? el : null;
}

/** S3: keeps the newest line of the stream in view without a scrollbar jump. */
async function scrollOutputToEnd() {
  await nextTick();
  const el = outputEl.value;
  if (el) el.scrollTop = el.scrollHeight;
}

/**
 * S4: asks pi for one provider's account state. A probe that could not run says
 * nothing about the account, so it lands as `unknown` instead of inventing a
 * login state nobody observed.
 */
async function refreshStatus(provider: string) {
  try {
    const status = await piAuthCheck(provider);
    statuses.value = { ...statuses.value, [provider]: status };
  } catch {
    statuses.value = {
      ...statuses.value,
      [provider]: { provider, state: "unknown", credential: false },
    };
  }
}

async function refreshAll(providers: string[]) {
  await Promise.all(providers.map((provider) => refreshStatus(provider)));
}

/**
 * S3: ends the live login, if any. Always safe to call: the session is dropped
 * from the UI first, and Rust treats a stop of an unknown id as `false`.
 */
async function stopLiveSession() {
  const live = session.value;
  session.value = null;
  loginRunning.value = false;
  loginFinished.value = false;
  loginExitCode.value = null;
  if (!live) return;
  try {
    await piCliStop(live.sessionId);
  } catch {
    // Best effort: the id is already forgotten here, and Rust kills every child
    // with the host anyway.
  }
}

/** S3: the streamed events of the one session this section owns. */
function handleCliEvent(event: PiCliEvent) {
  const live = session.value;
  // Events of an older session (or of another surface) are not ours to render.
  if (!live || event.cliSessionId !== live.sessionId) return;
  if (event.stream === "exit") {
    loginRunning.value = false;
    loginFinished.value = true;
    loginExitCode.value = event.exitCode ?? null;
    // Rust keeps a session registered until it is stopped, so reap the finished
    // child here instead of leaving an id in the registry for the whole app run.
    void piCliStop(live.sessionId).catch(() => {});
    // The login may have changed the account: ask pi again rather than guessing.
    void refreshStatus(live.provider);
    return;
  }
  if (event.line === undefined) return;
  loginLines.value = [...loginLines.value, event.line];
  void scrollOutputToEnd();
}

/**
 * S3: starts the device-code login for one provider. Only one login runs at a
 * time, so a second "Sign in" stops the previous flow first.
 */
async function startLogin(provider: string) {
  if (starting.value !== null) return;
  starting.value = provider;
  clearRowError(provider);
  try {
    await stopLiveSession();
    loginLines.value = [];
    loginCode.value = "";
    loginFinished.value = false;
    loginExitCode.value = null;
    const sessionId = await piCliStart([
      "auth",
      "login",
      provider,
      "--no-browser",
    ]);
    session.value = { provider, sessionId };
    loginRunning.value = true;
  } catch (cause) {
    setRowError(provider, commandErrorMessage(cause));
  } finally {
    starting.value = null;
  }
}

/** S3: sends the pasted code to the child's stdin (Rust appends the newline). */
async function submitCode() {
  const live = session.value;
  const code = loginCode.value.trim();
  if (!live || code === "" || sendingCode.value) return;
  sendingCode.value = true;
  clearRowError(live.provider);
  try {
    const accepted = await piCliWrite(live.sessionId, code);
    if (accepted) {
      // The child echoes the outcome on its own stream; the field is spent.
      loginCode.value = "";
    } else {
      setRowError(live.provider, t("settings.accounts.login.notAccepted"));
    }
  } catch (cause) {
    setRowError(live.provider, commandErrorMessage(cause));
  } finally {
    sendingCode.value = false;
  }
}

/** S3: "Cancel" really kills the child and closes the panel. */
async function cancelLogin() {
  await stopLiveSession();
  loginLines.value = [];
  loginCode.value = "";
}

/** S4: opens the sign-out confirmation for one provider. */
function requestSignOut(provider: string) {
  signOutTarget.value = provider;
  signOutOpen.value = true;
  clearRowError(provider);
}

/**
 * S4: signs out after confirmation and re-reads the state. A failure keeps the
 * reason on the row — pi's own message when it printed one — instead of closing
 * the flow silently.
 */
async function confirmSignOut() {
  const provider = signOutTarget.value;
  if (!provider || signingOut.value !== null) return;
  signOutOpen.value = false;
  signOutTarget.value = null;
  signingOut.value = provider;
  clearRowError(provider);
  try {
    const result = await piCliRun(
      ["auth", "logout", provider],
      LOGOUT_TIMEOUT_MS,
    );
    const detail = result.stderr.trim() || result.stdout.trim();
    if (result.timedOut) {
      setRowError(provider, t("settings.accounts.signOut.timeout"));
    } else if (result.exitCode !== 0) {
      setRowError(
        provider,
        detail ||
          (result.exitCode === null
            ? t("settings.accounts.signOut.failedUnknown")
            : t("settings.accounts.signOut.failed", {
                code: result.exitCode,
              })),
      );
    }
    // Even a failed logout may have cleared something: the badge is re-read.
    await refreshStatus(provider);
  } catch (cause) {
    setRowError(provider, commandErrorMessage(cause));
  } finally {
    signingOut.value = null;
  }
}

// S4: the states are read once per row set, so a provider list reload does not
// re-probe every account in a loop.
watch(
  () => rows.value.join("\n"),
  (key) => {
    void refreshAll(key === "" ? [] : key.split("\n"));
  },
  { immediate: true },
);

onMounted(async () => {
  const unsubscribe = await subscribePiCli(handleCliEvent);
  if (disposed) {
    unsubscribe();
    return;
  }
  unsubscribeEvents = unsubscribe;
});

onBeforeUnmount(() => {
  disposed = true;
  unsubscribeEvents?.();
  unsubscribeEvents = null;
  // S3: no login flow outlives the pane that started it.
  void stopLiveSession();
});
</script>

<template>
  <section class="space-y-2">
    <div class="min-w-0">
      <h3 class="text-sm font-medium">
        {{ t("settings.accounts.title") }}
      </h3>
      <p class="mt-0.5 text-xs text-muted-foreground">
        {{ t("settings.accounts.description") }}
      </p>
    </div>

    <!-- S5: rows are the configured providers plus the well-known subscription
         ids. No rule per row: the pane keeps its separators between blocks. -->
    <ul class="space-y-0.5">
      <li
        v-for="id in rows"
        :key="id"
        class="rounded-md px-2 py-1.5 transition-colors hover:bg-muted"
      >
        <div class="flex items-center gap-2">
          <span class="min-w-0 flex-1 truncate font-mono text-xs" :title="id">
            {{ id }}
          </span>
          <!-- S4: the badge is pi's own answer, never a guess. -->
          <Badge :variant="AUTH_STATE_VARIANTS[stateOf(id)]">
            {{ t(AUTH_STATE_KEYS[stateOf(id)]) }}
          </Badge>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            :disabled="starting === id || signingOut === id"
            @click="startLogin(id)"
          >
            {{
              starting === id
                ? t("settings.accounts.signInBusy")
                : t("settings.accounts.signIn")
            }}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            :disabled="signingOut === id || starting === id"
            @click="requestSignOut(id)"
          >
            {{
              signingOut === id
                ? t("settings.accounts.signOutBusy")
                : t("settings.accounts.signOut")
            }}
          </Button>
        </div>

        <!-- A rejected call keeps its reason on its own row. -->
        <p v-if="errors[id]" class="mt-1 text-xs text-destructive">
          {{ errors[id] }}
        </p>

        <!-- S3: the live device-code flow. pi prints the URL and the user code;
             the code the browser shows is pasted back here, to the child's
             stdin. Only one of these exists at a time. -->
        <div v-if="session?.provider === id" class="mt-2 space-y-2">
          <div
            :ref="bindOutput"
            role="log"
            aria-live="polite"
            class="max-h-40 overflow-y-auto whitespace-pre-wrap break-words rounded-md border border-border bg-muted/40 p-2 font-mono text-xs text-foreground"
          >
            <p v-for="(line, index) in loginLines" :key="index" class="m-0">
              {{ line }}
            </p>
            <p
              v-if="!loginLines.length"
              class="m-0 text-muted-foreground"
            >
              {{ t("settings.accounts.login.waiting") }}
            </p>
          </div>

          <p v-if="loginFinished" class="text-xs text-muted-foreground">
            {{
              loginExitCode === null
                ? t("settings.accounts.login.finished")
                : t("settings.accounts.login.finishedCode", {
                    code: loginExitCode,
                  })
            }}
          </p>

          <form
            class="flex flex-wrap items-center gap-2"
            @submit.prevent="submitCode"
          >
            <Input
              v-model="loginCode"
              class="h-8 min-w-0 flex-1 font-mono text-sm"
              autocomplete="off"
              spellcheck="false"
              :disabled="!loginRunning || sendingCode"
              :aria-label="t('settings.accounts.login.codeLabel', { id })"
              :placeholder="t('settings.accounts.login.codePlaceholder')"
            />
            <Button
              type="submit"
              size="sm"
              :disabled="
                loginCode.trim() === '' || sendingCode || !loginRunning
              "
            >
              {{
                sendingCode
                  ? t("settings.accounts.login.sending")
                  : t("settings.accounts.login.continue")
              }}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              :disabled="sendingCode"
              @click="cancelLogin"
            >
              {{ t("common.cancel") }}
            </Button>
          </form>

          <p class="text-xs text-muted-foreground">
            {{ t("settings.accounts.login.instructions") }}
          </p>
        </div>
      </li>
    </ul>

    <!-- S4: one confirmation dialog for the whole list, like the removal one:
         the row only says which provider it is about. -->
    <Dialog v-model:open="signOutOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            {{ t("settings.accounts.signOut.title") }}
          </DialogTitle>
          <DialogDescription>
            {{
              t("settings.accounts.signOut.description", {
                id: signOutTarget ?? "",
              })
            }}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <DialogClose as-child>
            <Button variant="outline">
              {{ t("common.cancel") }}
            </Button>
          </DialogClose>
          <Button
            variant="destructive"
            :disabled="signingOut !== null"
            @click="confirmSignOut"
          >
            {{ t("settings.accounts.signOut") }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </section>
</template>
