<script lang="ts">
/**
 * S2 of `providers-pane-refinements.md`: the account states are a session cache,
 * so they live at module scope and outlive the pane.
 *
 * One `pi auth check` boots a whole CLI (~1.15 s), so the badges are genuinely
 * background work and never a prerequisite of anything: the rows paint from the
 * fixed subscription list, the batch is scheduled a beat after the section is on
 * screen (and deferred to an idle callback when the webview has one), and only
 * then does a single `pi_auth_check_many` (bounded pool in Rust) run. Nothing in
 * the pane awaits it: the badges start as a neutral placeholder and fill in.
 *
 * The answer is kept here. Reopening the pane paints the cached badges at once
 * and probes nothing; "Refresh status" is the explicit way to ask pi again.
 *
 * Module scope is the point: this component is unmounted every time the pane is
 * left, and a `ref` inside it would die with the pane and re-probe on every open.
 * A scheduled probe belongs to the mount that scheduled it, so leaving the pane
 * cancels it and no batch ever runs behind a hidden section.
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
import { piAuthCheck, piAuthCheckMany } from "@/contracts/commands";
import type { PiAuthStatus } from "@/contracts/types";

/**
 * S3: the only providers this section lists. A subscription login exists for
 * these four and for nothing else: a configured endpoint (HelmCode, NaN, any
 * custom one) belongs to the Providers list, not here. The list is fixed on
 * purpose — a row exists so the account can be enabled, even when nothing
 * configured that provider yet.
 */
const SUBSCRIPTION_PROVIDERS = [
  "openai",
  "openai-codex",
  "anthropic",
  "google",
] as const;

/** S2: the states observed this session, by provider id. Empty means "not seen". */
const statuses = ref<Record<string, PiAuthStatus>>({});

/**
 * S2: how long the section waits after mount before the first probe. The delay
 * is the whole point of the unit: the rows are readable, the pane is switchable
 * and the first click lands before a single CLI boots.
 */
const PROBE_DELAY_MS = 1_200;

/**
 * S2: the deadline handed to `requestIdleCallback`. A busy frame would otherwise
 * defer the batch forever; at the latest the probe runs when this expires.
 */
const PROBE_IDLE_TIMEOUT_MS = 2_000;

/** S2: batches queued or running, so a badge with no cached state shows its pending face. */
const probing = ref(false);

/**
 * S2: how many batches are queued or running. The pending face follows this
 * counter, so the placeholder does not blink between the two waves.
 */
let queuedBatches = 0;

/** The tail of the queue: waves run one after another, never in parallel. */
let queue: Promise<void> = Promise.resolve();

/**
 * S2: one batched probe. A provider that could not be probed lands as `unknown`
 * — the honest word for "nobody observed this account" — never as a guess.
 * Providers that already have a state keep it unless the batch answers again.
 */
async function runBatch(providers: string[]): Promise<void> {
  try {
    const answer = await piAuthCheckMany(providers);
    const next = { ...statuses.value };
    for (const status of answer) next[status.provider] = status;
    statuses.value = next;
  } catch {
    const next = { ...statuses.value };
    for (const provider of providers) {
      next[provider] = { provider, state: "unknown", credential: false };
    }
    statuses.value = next;
  }
}

/**
 * S2: queues one batch behind whatever is already running, so two waves and an
 * explicit refresh never spawn two CLI pools at once, and the pending face stays
 * on screen until the last queued batch has landed.
 */
function probeMany(providers: string[]): Promise<void> {
  if (providers.length === 0) return Promise.resolve();
  queuedBatches += 1;
  probing.value = true;
  const run = queue.then(() => runBatch(providers));
  const settle = () => {
    queuedBatches -= 1;
    probing.value = queuedBatches > 0;
  };
  queue = run.then(settle, settle);
  return run;
}

/** S2: the single-provider re-read a finished sign-in or sign-out asks for. */
async function probeOne(provider: string): Promise<void> {
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
</script>

<script setup lang="ts">
/**
 * Subscription accounts (specs S3, S4, S5 of `odd/tasks/pi-cli-bridge.md`, S2/S3
 * of `odd/tasks/providers-pane-refinements.md`).
 *
 * S5: the parent renders this section only when `VITE_ENABLE_PROVIDER_LOGIN` is
 * on. The flag is read by `SettingsDialog.vue` with the same `readFlag` helper as
 * `VITE_ENABLE_CUSTOM_PROVIDERS` (on by default, only an explicit
 * `0`/`false`/`off` turns it off), because that is where every build-time switch
 * of the dialog lives; this component never declares a second copy of the rule.
 *
 * S3 of `providers-pane-refinements.md`: the rows are exactly the four providers
 * a subscription can sign in to (`openai`, `openai-codex`, `anthropic`,
 * `google`) — never pi's configured providers. A configured endpoint (HelmCode,
 * NaN, a custom one) is a Providers row, not an account; and a subscription
 * provider nobody configured still gets a row, because signing in is what makes
 * it exist at all.
 *
 * S3: "Sign in" is the pi subscription login, not an api key. It starts
 * `pi auth login <provider> --no-browser` through the Rust bridge, which is the
 * device-code flow: pi prints a URL and a user code, the human approves it in a
 * browser, and the code pi shows is pasted back here. The child's output is
 * streamed live (`pi-cli` events) and the pasted code goes to its stdin.
 * There is only ever one login at a time: starting another stops the previous
 * one, and leaving this pane stops whatever is live.
 *
 * S1 of `models-by-provider.md`: the real OpenAI login completes by itself on
 * the local callback (`127.0.0.1:1455`), so the default face of a login is one
 * short line and a way out. The streamed output, the extracted URL, the
 * "Open in browser" button and the paste field are backup, and they live behind
 * a disclosure the human opens on purpose. A finished login collapses to its
 * outcome: one confirmation on success, the last meaningful line on failure.
 *
 * S4: the badge is pi's own answer — never a guess — and "Sign out" runs
 * `pi auth logout <provider>` behind a confirmation. Credentials stay in pi's
 * `auth.json`: this component never sees a secret, only a state word.
 *
 * S2 of `providers-pane-refinements.md`: asking for a badge means booting a
 * whole CLI, so the badges are read in the background, in one batch, and cached
 * for the session (see the module block above). "Refresh status" is the only
 * other way to ask, and nothing in the pane waits for an answer.
 *
 * Sign-in URL: pi prints "Open this URL to sign in:" followed by the OAuth URL
 * whose callback points at `127.0.0.1:1455`. That URL is read out of the streamed
 * lines, shown on its own block above the paste field and opened in the system
 * browser exactly once per login session. Nothing is remembered between sessions:
 * the URL only lives as long as the child that printed it.
 *
 * Layout: rows carry no rule of their own (no `border-b`), and this section adds
 * no separator at all — the pane's rules only sit between major blocks.
 */
import { ChevronRightIcon } from "@lucide/vue";
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
  openExternalUrl,
  piCliRun,
  piCliStart,
  piCliStop,
  piCliWrite,
  subscribePiCli,
} from "@/contracts/commands";
import type { PiAuthState, PiCliEvent } from "@/contracts/types";

/** S3/S4: every visible string, both locales (S2 of `i18n-and-settings-polish.md`). */
const { t } = useI18n();

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
 * S3: any absolute `http(s)` URL in a streamed line. The device-code flow prints
 * the OAuth URL on its own line, and terminals/pi may wrap it in punctuation, so
 * the match stops at whitespace or a quote and the tail is trimmed below.
 */
const URL_PATTERN = /https?:\/\/[^\s<>"'`]+/g;

/** `)` `.` `,` and friends close a sentence, not a URL: they never travel to the browser. */
const TRAILING_PUNCTUATION = /[)\]}.,;:!'"»”]+$/;

/**
 * S3: what a sign-in link looks like. An OAuth URL names its own purpose in the
 * path (`/oauth/authorize`, `/login`, device flows) or only in its query, so the
 * whole URL is searched. It is a preference, not a filter: a line with no such
 * word still yields its first well-formed URL, because guessing wrong is better
 * than leaving the human to hunt for a link pi printed.
 */
const SIGN_IN_HINT =
  /(auth|oauth|log-?in|sign-?in|authoriz|device|consent|verify)/i;

/**
 * S3: the URL a browser can actually receive. Parsing it is also the guard that
 * a truncated or mangled line never reaches the system opener.
 */
function normalizeUrl(raw: string): string | null {
  const trimmed = raw.replace(TRAILING_PUNCTUATION, "");
  try {
    const parsed = new URL(trimmed);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
    if (parsed.hostname === "") return null;
    return trimmed;
  } catch {
    return null;
  }
}

/**
 * S3: the first sign-in-looking URL of the streamed lines, `null` until pi prints
 * one. The first match is final for the session: the URL pi generates does not
 * change while the child waits for the callback.
 */
function extractSignInUrl(lines: string[]): string | null {
  let fallback: string | null = null;
  for (const line of lines) {
    for (const match of line.matchAll(URL_PATTERN)) {
      const url = normalizeUrl(match[0]);
      if (url === null) continue;
      if (SIGN_IN_HINT.test(url)) return url;
      fallback ??= url;
    }
  }
  return fallback;
}

/** S3: the rows are the subscription providers, fixed — the pane's providers are elsewhere. */
const rows = SUBSCRIPTION_PROVIDERS;

/**
 * S2: the subscription providers pi already has in `models.json`, handed over by
 * the pane from the list it renders. They are the first probe wave: an account
 * the human can already see configured is usually the one they came to check.
 */
const props = defineProps<{ configured: string[] }>();

/**
 * A sign-in just finished successfully, so pi may have refreshed its cached model
 * catalogue (`models-store.json`) on the way. The pane re-reads the providers it
 * lists and its open editor re-reads the models: nothing is guessed here, the
 * file is simply read again.
 */
const emit = defineEmits<{ signedIn: [provider: string] }>();

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
/**
 * S3: the sign-in URL pi just printed, `null` while it has not printed one. It is
 * never stored and never restored: it belongs to the live child only.
 */
const signInUrl = ref<string | null>(null);
/**
 * S3: the session id whose URL was already handed to the browser. Auto-open happens
 * once per login session even if the same URL shows up on several lines; a manual
 * "Open in browser" click is a user action and is not counted here.
 */
const autoOpenedFor = ref<string | null>(null);
/** The system opener rejected the URL, so the block says to copy it instead. */
const openFailed = ref(false);
/**
 * S1: the "Trouble signing in?" disclosure, closed by default. It is per login
 * session on purpose: once the human opens it, later lines of the same login do
 * not fold it again, and the next login starts quiet.
 */
const detailsOpen = ref(false);

/**
 * S1: the longest failure line quoted on the status line. The rest is detail and
 * stays in the disclosure; a wall of text is exactly what this unit removes.
 */
const FAILURE_LINE_MAX = 160;

/** S1: the three faces of a login, in the order they supersede each other. */
type LoginPhase = "running" | "succeeded" | "failed";

/**
 * S1: exits 0 is success; any other end — a code, a signal, a killed child — is
 * a failure and keeps its reason. The child is still `running` until it exits.
 */
const loginPhase = computed<LoginPhase>(() => {
  if (!loginFinished.value) return "running";
  return loginExitCode.value === 0 ? "succeeded" : "failed";
});

/**
 * S1: what the running line says. A login whose browser never opened would wait
 * forever in silence, so that one fallback replaces the message and points at
 * the disclosure instead of dumping the URL on the row.
 */
const runningStatus = computed<string>(() =>
  openFailed.value
    ? t("settings.accounts.login.openManually")
    : t("settings.accounts.login.waitingBrowser"),
);

/** S1: the last non-blank line pi printed, the only one worth quoting. */
function lastMeaningfulLine(lines: string[]): string | null {
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    const line = (lines[index] ?? "").trim();
    if (line !== "") return line;
  }
  return null;
}

/** S1: short enough for one line; the disclosure keeps whatever it cut. */
function truncateLine(line: string): string {
  return line.length <= FAILURE_LINE_MAX
    ? line
    : `${line.slice(0, FAILURE_LINE_MAX - 1)}…`;
}

/** S1: the failure line — pi's own last word, or the exit code when it said none. */
const failureLine = computed<string>(() => {
  const line = lastMeaningfulLine(loginLines.value);
  if (line !== null) return line;
  const code = loginExitCode.value;
  return code === null
    ? t("settings.accounts.login.failed")
    : t("settings.accounts.login.failedWithCode", { code });
});

/** S1: the single line a login shows, whichever face it is in. */
const statusText = computed<string>(() => {
  switch (loginPhase.value) {
    case "running":
      return runningStatus.value;
    case "succeeded":
      return t("settings.accounts.login.succeeded");
    case "failed":
      return truncateLine(failureLine.value);
  }
});

/** S1: the untruncated failure stays reachable without widening the row. */
const statusTitle = computed<string>(() =>
  loginPhase.value === "failed" ? failureLine.value : statusText.value,
);

/** S1: semantic tone only — success, failure and the manual fallback explain themselves. */
const statusClass = computed<string>(() => {
  switch (loginPhase.value) {
    case "succeeded":
      return "text-success";
    case "failed":
      return "text-destructive";
    default:
      return openFailed.value ? "text-warning" : "text-muted-foreground";
  }
});

/** S1: one disclosure per row, so `aria-controls` points at this login's panel. */
function detailsPanelId(provider: string): string {
  return `login-details-${provider}`;
}

/** S1: the human decides when the backup controls appear. */
function toggleDetails() {
  detailsOpen.value = !detailsOpen.value;
}

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

/**
 * S2: this row was never observed — the batch is scheduled, on its way, or the
 * cached states are from an earlier mount. The badge says "checking" instead of
 * claiming the account is unknown, and nothing in the pane waits for the answer.
 * Sign in, sign out and the rest of the pane stay usable while it fills in.
 */
function isUnobserved(provider: string): boolean {
  return statuses.value[provider] === undefined;
}

/**
 * S2: the explicit re-probe behind "Refresh status". It asks for all four in one
 * call; the cached badges stay on screen while the answer travels.
 */
function refreshStatuses() {
  void probeMany([...SUBSCRIPTION_PROVIDERS]);
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
 * S3: ends the live login, if any. Always safe to call: the session is dropped
 * from the UI first, and Rust treats a stop of an unknown id as `false`.
 */
async function stopLiveSession() {
  const live = session.value;
  session.value = null;
  loginRunning.value = false;
  loginFinished.value = false;
  loginExitCode.value = null;
  // The URL belonged to the child that just went away: no link outlives it.
  signInUrl.value = null;
  autoOpenedFor.value = null;
  openFailed.value = false;
  detailsOpen.value = false;
  if (!live) return;
  try {
    await piCliStop(live.sessionId);
  } catch {
    // Best effort: the id is already forgotten here, and Rust kills every child
    // with the host anyway.
  }
}

/**
 * S3: hands one URL to the system browser. A rejection is not fatal: the block
 * stays on screen with the URL, which is the manual fallback.
 */
async function openInBrowser(url: string) {
  openFailed.value = false;
  try {
    await openExternalUrl(url);
  } catch {
    openFailed.value = true;
  }
}

/**
 * S3: the user asked for it again. Only the URL the current child printed can be
 * opened, so a finished session has nothing left to hand to the browser.
 */
function openSignInUrl() {
  const url = signInUrl.value;
  if (url === null) return;
  void openInBrowser(url);
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
    // The sign-in is over, so the link is spent; the streamed lines stay on
    // screen as the outcome pi printed last.
    signInUrl.value = null;
    openFailed.value = false;
    // S1: a login that worked leaves one confirmation and nothing to read, so
    // the transcript of a successful child is dropped here. A failed one keeps
    // its lines: they are the detail behind the quoted error.
    if (event.exitCode === 0) {
      loginLines.value = [];
      // A successful sign-in is what makes pi write its cached catalogue, so the
      // pane is told and re-reads the providers instead of showing a list that
      // predates the login.
      emit("signedIn", live.provider);
    }
    // Rust keeps a session registered until it is stopped, so reap the finished
    // child here instead of leaving an id in the registry for the whole app run.
    void piCliStop(live.sessionId).catch(() => {});
    // The login may have changed the account: ask pi again rather than guessing.
    void probeOne(live.provider);
    return;
  }
  if (event.line === undefined) return;
  loginLines.value = [...loginLines.value, event.line];
  if (detailsOpen.value) void scrollOutputToEnd();
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
    signInUrl.value = null;
    autoOpenedFor.value = null;
    openFailed.value = false;
    detailsOpen.value = false;
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

/**
 * S1: drops the outcome of a login that already ended. The child is gone by
 * then (exit reaps it), so this is a view reset and never a second stop.
 */
function dismissLogin() {
  session.value = null;
  loginLines.value = [];
  loginCode.value = "";
  loginRunning.value = false;
  loginFinished.value = false;
  loginExitCode.value = null;
  signInUrl.value = null;
  autoOpenedFor.value = null;
  openFailed.value = false;
  detailsOpen.value = false;
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
    await probeOne(provider);
  } catch (cause) {
    setRowError(provider, commandErrorMessage(cause));
  } finally {
    signingOut.value = null;
  }
}

// S3: the sign-in URL is read out of the stream as it arrives and kept for the
// rest of the session. Re-reading the whole stream is cheap (pi prints a handful
// of lines) and keeps the extraction free of parse state.
watch(
  () => loginLines.value,
  (lines) => {
    if (signInUrl.value !== null) return;
    const found = extractSignInUrl(lines);
    if (found !== null) signInUrl.value = found;
  },
);

// S1: the stream follows its newest line, but only while the human has the
// disclosure open; a closed panel does no work and never scrolls.
watch(detailsOpen, (open) => {
  if (open) void scrollOutputToEnd();
});

// S3: auto-open, at most once per login session. The guard is the session id, so
// a re-printed URL, a re-render or a second watcher tick cannot open the browser
// again; a new login has a new id and therefore earns its own single open.
watch(signInUrl, (url) => {
  const live = session.value;
  if (url === null || live === null) return;
  if (autoOpenedFor.value === live.sessionId) return;
  autoOpenedFor.value = live.sessionId;
  void openInBrowser(url);
});

/**
 * S2: the two waves of the background fill. Wave 1 is the subscription providers
 * that already exist in `models.json`; wave 2 is the rest of the fixed list. Both
 * go through the batched wrapper, so each wave is ONE CLI pool — the useful
 * accounts land in ~1 s instead of after the whole list.
 */
function scheduleProbe() {
  const configured = new Set(props.configured);
  const unseen = (ids: readonly string[]) =>
    ids.filter((id) => statuses.value[id] === undefined);
  const firstWave = unseen(
    SUBSCRIPTION_PROVIDERS.filter((id) => configured.has(id)),
  );
  const secondWave = unseen(
    SUBSCRIPTION_PROVIDERS.filter((id) => !configured.has(id)),
  );
  if (firstWave.length === 0 && secondWave.length === 0) return;
  void (async () => {
    if (firstWave.length > 0) await probeMany(firstWave);
    if (disposed) return;
    // Re-filtered: a sign-out or a refresh may have answered while wave 1 ran.
    const rest = unseen(secondWave);
    if (rest.length > 0) await probeMany(rest);
  })();
}

/** S2: the scheduled fill of this mount, so leaving the pane cancels it. */
let probeTimer: ReturnType<typeof setTimeout> | null = null;
let idleHandle: number | null = null;

/** S2: nothing scheduled may outlive the section that scheduled it. */
function cancelScheduledProbe() {
  if (probeTimer !== null) {
    clearTimeout(probeTimer);
    probeTimer = null;
  }
  if (idleHandle !== null) {
    if (typeof cancelIdleCallback === "function") {
      cancelIdleCallback(idleHandle);
    }
    idleHandle = null;
  }
}

/**
 * S2: the delay expired. The waves run in an idle callback when the webview has
 * one — so the batch never competes with a paint or an input frame — and fall
 * back to the timer alone when it does not.
 */
function deferProbe() {
  probeTimer = null;
  if (disposed) return;
  if (typeof requestIdleCallback === "function") {
    idleHandle = requestIdleCallback(
      () => {
        idleHandle = null;
        if (disposed) return;
        scheduleProbe();
      },
      { timeout: PROBE_IDLE_TIMEOUT_MS },
    );
    return;
  }
  scheduleProbe();
}

onMounted(async () => {
  const unsubscribe = await subscribePiCli(handleCliEvent);
  if (disposed) {
    unsubscribe();
    return;
  }
  unsubscribeEvents = unsubscribe;
});

/**
 * S2: mount schedules the badges and awaits nothing else. The section exists only
 * while it is on screen (the pane renders it with `v-if`), so a hidden section
 * schedules no probe at all — and one timer, cancelled on unmount, is the whole
 * cost of a pane the human leaves immediately.
 */
onMounted(() => {
  probeTimer = setTimeout(deferProbe, PROBE_DELAY_MS);
});

onBeforeUnmount(() => {
  disposed = true;
  cancelScheduledProbe();
  unsubscribeEvents?.();
  unsubscribeEvents = null;
  // S3: no login flow outlives the pane that started it.
  void stopLiveSession();
});
</script>

<template>
  <section class="space-y-2">
    <div class="flex flex-wrap items-start justify-between gap-2">
      <div class="min-w-0">
        <h3 class="text-sm font-medium">
          {{ t("settings.accounts.title") }}
        </h3>
        <p class="mt-0.5 text-xs text-muted-foreground">
          {{ t("settings.accounts.description") }}
        </p>
      </div>
      <!-- S2: the only way to ask pi again once the states are cached. It never
           disables the rows: a probe in flight is visible on the badges. -->
      <Button
        type="button"
        variant="ghost"
        size="sm"
        class="shrink-0"
        :disabled="probing"
        @click="refreshStatuses"
      >
        {{
          probing
            ? t("settings.accounts.refreshing")
            : t("settings.accounts.refresh")
        }}
      </Button>
    </div>

    <!-- S3: the rows are the four subscription providers, fixed. No rule per
         row: the pane keeps its separator between blocks. -->
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
          <!-- S4: the badge is pi's own answer, never a guess. S2: a row that was
               never observed shows the neutral placeholder — pulsing while the
               batch is queued or on its way — and never blocks anything. -->
          <Badge
            v-if="isUnobserved(id)"
            variant="outline"
            class="text-muted-foreground"
            :class="probing ? 'animate-pulse' : ''"
          >
            {{ t("settings.accounts.state.checking") }}
          </Badge>
          <Badge v-else :variant="AUTH_STATE_VARIANTS[stateOf(id)]">
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

        <!-- S1: the live sign-in. Its default face is one line — what it waits
             for, and the way out — because the real flow finishes by itself on
             the local callback. Only one of these exists at a time. -->
        <div v-if="session?.provider === id" class="mt-2 space-y-1.5">
          <div class="flex items-center gap-2">
            <span
              v-if="loginPhase === 'running'"
              class="size-3 shrink-0 animate-spin rounded-full border-2 border-muted-foreground/30 border-t-muted-foreground"
              aria-hidden="true"
            />
            <p
              role="status"
              aria-live="polite"
              class="min-w-0 flex-1 truncate text-xs"
              :class="statusClass"
              :title="statusTitle"
            >
              {{ statusText }}
            </p>
            <Button
              v-if="loginPhase === 'running'"
              type="button"
              variant="ghost"
              size="sm"
              class="h-6 shrink-0 px-2 text-xs"
              @click="cancelLogin"
            >
              {{ t("common.cancel") }}
            </Button>
            <Button
              v-else
              type="button"
              variant="ghost"
              size="sm"
              class="h-6 shrink-0 px-2 text-xs"
              @click="dismissLogin"
            >
              {{ t("common.dismiss") }}
            </Button>
          </div>

          <!-- S1: the backup path — stream, URL, browser button and paste field —
               is detail. It is closed until the human asks for it, stays open
               for the rest of this login, and a successful one has nothing left
               to disclose. -->
          <div v-if="loginPhase !== 'succeeded'">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              class="h-6 px-1 text-xs font-normal text-muted-foreground"
              :aria-expanded="detailsOpen"
              :aria-controls="detailsPanelId(id)"
              @click="toggleDetails"
            >
              <ChevronRightIcon
                class="size-3 shrink-0 transition-transform"
                :class="detailsOpen ? 'rotate-90' : ''"
                aria-hidden="true"
              />
              {{ t("settings.accounts.login.disclosure") }}
            </Button>

            <!-- S1: `v-show` keeps the panel in the document, so the disclosure
                 button's `aria-controls` always resolves; a hidden node is not a
                 live region, so nothing is announced until it is opened. -->
            <div
              v-show="detailsOpen"
              :id="detailsPanelId(id)"
              class="mt-1.5 space-y-2"
            >
              <div
                :ref="bindOutput"
                role="log"
                :aria-label="t('settings.accounts.login.output')"
                class="max-h-40 overflow-y-auto whitespace-pre-wrap break-words rounded-md border border-border bg-muted/40 p-2 font-mono text-xs text-foreground"
              >
                <p v-for="(line, index) in loginLines" :key="index" class="m-0">
                  {{ line }}
                </p>
                <p v-if="!loginLines.length" class="m-0 text-muted-foreground">
                  {{ t("settings.accounts.login.waitingBrowser") }}
                </p>
              </div>

              <!-- S3: the URL pi printed, standing on its own so nobody has to
                   hunt for it inside the stream. It was opened once
                   automatically; the button is the manual fallback, and the
                   whole block disappears with the child that printed it. -->
              <div
                v-if="signInUrl"
                class="space-y-1.5 rounded-md border border-primary/40 bg-primary/5 p-2"
              >
                <p class="text-xs font-medium">
                  {{ t("settings.accounts.login.urlTitle") }}
                </p>
                <p
                  class="truncate font-mono text-xs text-foreground select-all"
                  :title="signInUrl"
                >
                  {{ signInUrl }}
                </p>
                <Button
                  type="button"
                  size="sm"
                  variant="secondary"
                  class="h-7"
                  @click="openSignInUrl"
                >
                  {{ t("settings.accounts.login.openBrowser") }}
                </Button>
                <p v-if="openFailed" class="text-xs text-destructive">
                  {{ t("settings.accounts.login.openFailed") }}
                </p>
              </div>

              <p v-if="signInUrl" class="text-xs text-muted-foreground">
                {{ t("settings.accounts.login.callbackHint") }}
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
              </form>
            </div>
          </div>
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
