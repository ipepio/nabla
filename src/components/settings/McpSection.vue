<script setup lang="ts">
/**
 * MCP servers pane (S1 of `odd/tasks/tool-drawer-and-mcp.md`).
 *
 * One list, one write path per action, and no state the file does not hold:
 *
 * - the rows come from `mcp_list()`, which reads pi's own `mcp.json` files and
 *   connects the enabled servers once (`pi mcp list --json`). A server that
 *   cannot connect is a row with `state: "error"` and pi's reason under it, so
 *   one broken entry never hides the others. `unknown` is the honest badge for a
 *   state word this build does not model.
 * - every action answers with the whole refreshed list (`mcp_remove`,
 *   `mcp_logout`, `mcp_add`), so the pane renders what the file now holds instead
 *   of patching a row it did not write.
 * - signing in is the provider login's flow, not a second one: `mcp_login_start`
 *   spawns `pi mcp login`, the child's output arrives on the existing `pi-cli`
 *   event, the sign-in URL is read out of that stream and opened once, and the
 *   pasted redirect URL goes back to the child's stdin through `piCliWrite`.
 *   Cancel is `piCliStop`. When the session exits the list is refreshed, because
 *   that exit is what changed pi's stored credential.
 *
 * One action per row, the rule the accounts block already follows: `ready` is the
 * only state that holds a credential, so it is the only one offering "Sign out";
 * every other state offers "Sign in". Removal is a separate, confirmed action
 * because it deletes the entry rather than its credential.
 *
 * The two `mcp.json` files this pane describes are shown by path, and "show file"
 * reveals one in the system file manager. Nothing of their content is read into
 * the dialog: `pi mcp list` is the summary, and the files stay pi's.
 *
 * Mounting never waits for that read. `pi mcp list --json` boots the CLI and
 * connects every enabled server (~0.7 s measured), and awaiting it on mount left
 * the pane saying "No MCP servers configured yet" for most of a second — the one
 * sentence that is wrong on a machine that has servers. So the list lives in
 * `src/lib/mcp-servers.ts`: this pane paints whatever that store already holds in
 * the first frame, schedules the read a beat later (an idle callback, the same
 * spirit as the accounts block) and asks for a newer answer only when the human
 * presses Refresh, a sign-in ends or a write lands. The store keeps the previous
 * rows on a failed read, and the `settled` flag is what tells an empty answer
 * apart from no answer at all: the empty state is claimed only by a settled read
 * that genuinely found nothing.
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { ChevronRightIcon, ExternalLinkIcon, PlusIcon } from "@lucide/vue";
import { Alert, AlertDescription } from "@/components/ui/alert";
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
import { Separator } from "@/components/ui/separator";
import { useI18n, type MessageKey } from "@/lib/i18n";
import {
  applyMcpServers,
  error as serversError,
  invalidateMcpServers,
  loadMcpServers,
  loading as serversLoading,
  servers,
  settled as serversSettled,
} from "@/lib/mcp-servers";
import {
  commandErrorMessage,
  mcpFilePath,
  mcpLoginStart,
  mcpLogout,
  mcpRemove,
  openExternalUrl,
  piCliStop,
  piCliWrite,
  revealFileInFileManager,
  subscribePiCli,
} from "@/contracts/commands";
import type {
  McpScope,
  McpServerSummary,
  McpState,
  PiCliEvent,
} from "@/contracts/types";
import McpServerForm from "./McpServerForm.vue";

const { t } = useI18n();

/** The scopes an `mcp.json` can live in, in the order the files block lists them. */
const SCOPES: McpScope[] = ["global", "project"];

/**
 * How each state is painted (S1): ready is the success tone, a sign-in demand is
 * the warning tone, a failure is destructive, and a server that is off or
 * unobserved stays muted. The `Badge` variants have no success/warning faces, so
 * the two semantic colours come from the token classes the palette exposes.
 */
const STATE_BADGE: Record<
  McpState,
  { variant: NonNullable<BadgeVariants["variant"]>; class: string; key: MessageKey }
> = {
  ready: {
    variant: "outline",
    class: "border-success/40 text-success",
    key: "settings.mcp.state.ready",
  },
  needs_sign_in: {
    variant: "outline",
    class: "border-warning/40 text-warning",
    key: "settings.mcp.state.needsSignIn",
  },
  error: {
    variant: "destructive",
    class: "",
    key: "settings.mcp.state.error",
  },
  disabled: {
    variant: "outline",
    class: "text-muted-foreground",
    key: "settings.mcp.state.disabled",
  },
  unknown: {
    variant: "outline",
    class: "text-muted-foreground",
    key: "settings.mcp.state.unknown",
  },
};

/** The badge of a row, defaulting to the honest `unknown` for a newer state word. */
function badgeOf(state: McpState) {
  return STATE_BADGE[state] ?? STATE_BADGE.unknown;
}

/** The scope labels, both locales. */
const SCOPE_KEYS: Record<McpScope, MessageKey> = {
  global: "settings.mcp.scope.global",
  project: "settings.mcp.scope.project",
};

/**
 * Any absolute `http(s)` URL in a streamed line. `pi mcp login` prints the
 * authorization URL on its own line; terminals and pi may wrap it in punctuation,
 * so the match stops at whitespace or a quote and the tail is trimmed below.
 */
const URL_PATTERN = /https?:\/\/[^\s<>"'`]+/g;

/** `)` `.` `,` and friends close a sentence, not a URL. */
const TRAILING_PUNCTUATION = /[)\]}.,;:!'"»”]+$/;

/**
 * What an authorization link looks like. It is a preference, not a filter: a line
 * with no such word still yields its first well-formed URL, because guessing
 * wrong is better than leaving the human to hunt for a link pi printed.
 */
const SIGN_IN_HINT =
  /(auth|oauth|log-?in|sign-?in|authoriz|consent|verify|device)/i;

/** A trimmed URL that a browser can actually receive, `null` when it is not one. */
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
 * The sign-in URL of a stream, `null` until one appears. The first match is final
 * for the session: the URL the authorization server issued does not change while
 * the child waits for the callback.
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

/**
 * A re-read this pane asked for is on its way and the rows on screen are the
 * previous answer. The shared store owns `loading`; this flag is only "the human
 * (or a finished sign-in) asked again", so the Refresh action can say so.
 */
const refreshing = ref(false);
/** The pane's one write error line: an add, remove or sign-out that could not run. */
const error = ref<string | null>(null);
/**
 * Inline failures per row, keyed by `scope:name`. A sign-in that could not start
 * belongs to the server it was about, not to the bottom of a pane that is taller
 * than the dialog — the same rule the accounts block follows.
 */
const rowErrors = ref<Record<string, string>>({});
/** `scope:name` of the row whose action is in flight, `null` when idle. */
const busy = ref<string | null>(null);
/** S1: the inline add form is open. */
const showForm = ref(false);

/** The absolute path of each scope's file, resolved by Rust. */
const filePaths = ref<Record<McpScope, string | null>>({
  global: null,
  project: null,
});
/** A path that could not be resolved keeps its reason here, per scope. */
const fileErrors = ref<Record<string, string>>({});

/** The one live sign-in, `null` when there is none. */
const session = ref<{
  name: string;
  scope: McpScope;
  sessionId: string;
} | null>(null);
/** The streamed output of the live sign-in, oldest line first. */
const loginLines = ref<string[]>([]);
/** The redirect URL the human pastes when the browser is on another machine. */
const pastedUrl = ref("");
/** A write to the live session's stdin is in flight. */
const sendingUrl = ref(false);
/** The child is alive and can still read a URL. */
const loginRunning = ref(false);
/** The child left on its own. */
const loginFinished = ref(false);
/** Its exit code, `null` when a signal killed it (or while it runs). */
const loginExitCode = ref<number | null>(null);
/** The server whose sign-in is being started, `null` when idle. */
const starting = ref<string | null>(null);
/** The server whose sign-out is running, `null` when idle. */
const signingOut = ref<string | null>(null);
/** The authorization URL pi printed, opened once per session. */
const signInUrl = ref<string | null>(null);
/** The session id whose URL already reached the browser. */
const autoOpenedFor = ref<string | null>(null);
/** The system opener rejected the URL, so the block says to copy it instead. */
const openFailed = ref(false);
/** The "details" disclosure of the live sign-in, closed until asked. */
const detailsOpen = ref(false);
/** The longest failure line quoted on the status line. */
const FAILURE_LINE_MAX = 160;

/** The removal confirmation and its target. */
const removalOpen = ref(false);
const removalTarget = ref<McpServerSummary | null>(null);
/** The sign-out confirmation and its target. */
const signOutOpen = ref(false);
const signOutTarget = ref<McpServerSummary | null>(null);

/** The unsubscribe handle of the `pi-cli` listener, `null` until it resolves. */
let unsubscribeEvents: (() => void) | null = null;
/** Set on unmount, so a listener that resolves late is dropped instead of kept. */
let disposed = false;

/** The key of a row: one name can exist once per scope, so both halves matter. */
function rowKey(server: { name: string; scope: McpScope }): string {
  return `${server.scope}:${server.name}`;
}

function setRowError(key: string, message: string) {
  rowErrors.value = { ...rowErrors.value, [key]: message };
}

function clearRowError(key: string) {
  if (!(key in rowErrors.value)) return;
  const next = { ...rowErrors.value };
  delete next[key];
  rowErrors.value = next;
}

/**
 * S1: `ready` is the only state that says pi holds a credential for this server,
 * so it is the only one with something to sign out of. Every other answer — a
 * failed connection, a demand for OAuth, a disabled entry, an unmodelled state —
 * describes a row that may need a sign-in and offers "Sign in" alone.
 */
function offersSignOut(server: McpServerSummary): boolean {
  return server.state === "ready";
}

/** The status line of a sign-in, in the three faces it moves through. */
type LoginPhase = "running" | "succeeded" | "failed";

const loginPhase = computed<LoginPhase>(() => {
  if (!loginFinished.value) return "running";
  return loginExitCode.value === 0 ? "succeeded" : "failed";
});

/** The last non-blank line pi printed. */
function lastMeaningfulLine(lines: string[]): string | null {
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    const line = (lines[index] ?? "").trim();
    if (line !== "") return line;
  }
  return null;
}

function truncateLine(line: string): string {
  return line.length <= FAILURE_LINE_MAX
    ? line
    : `${line.slice(0, FAILURE_LINE_MAX - 1)}…`;
}

/** pi's own last word, or the exit code when it said nothing. */
const failureLine = computed<string>(() => {
  const line = lastMeaningfulLine(loginLines.value);
  if (line !== null) return line;
  const code = loginExitCode.value;
  return code === null
    ? t("settings.mcp.login.failed")
    : t("settings.mcp.login.failedWithCode", { code });
});

/** The single line the running sign-in shows. */
const statusText = computed<string>(() => {
  switch (loginPhase.value) {
    case "running":
      return openFailed.value
        ? t("settings.mcp.login.openManually")
        : t("settings.mcp.login.waitingBrowser");
    case "succeeded":
      return t("settings.mcp.login.succeeded");
    case "failed":
      return truncateLine(failureLine.value);
  }
});

/** The untruncated failure stays reachable without widening the row. */
const statusTitle = computed<string>(() =>
  loginPhase.value === "failed" ? failureLine.value : statusText.value,
);

/** Semantic tone only: the words explain the rest. */
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

/** One disclosure per row, so `aria-controls` points at this sign-in's panel. */
function detailsPanelId(server: McpServerSummary): string {
  return `mcp-login-details-${server.scope}-${server.name}`;
}

/** The output block, scrolled to the newest line as it arrives. */
const outputEl = ref<HTMLElement | null>(null);

function bindOutput(el: unknown) {
  outputEl.value = el instanceof HTMLElement ? el : null;
}

async function scrollOutputToEnd() {
  await nextTick();
  const el = outputEl.value;
  if (el) el.scrollTop = el.scrollHeight;
}

/**
 * S1: asks the shared store for a newer listing. `pi mcp list` connects every
 * enabled server, so an answer can take a while: the previous rows stay on
 * screen for all of it, and the store paints the new ones when it lands. A
 * failed read is reported by the store (see `serversError`), never by blanking
 * the list.
 */
async function refresh() {
  refreshing.value = true;
  try {
    await loadMcpServers({ force: true });
  } finally {
    refreshing.value = false;
  }
}

/**
 * S1: resolves both `mcp.json` paths. A project file that does not exist yet (or
 * an untrusted project) is not an error for the pane: it is one row that says the
 * path is unknown, while the other keeps working.
 */
async function loadFilePaths() {
  await Promise.all(
    SCOPES.map(async (scope) => {
      try {
        const path = await mcpFilePath(scope);
        filePaths.value = { ...filePaths.value, [scope]: path };
      } catch (cause) {
        filePaths.value = { ...filePaths.value, [scope]: null };
        fileErrors.value = {
          ...fileErrors.value,
          [scope]: commandErrorMessage(cause),
        };
      }
    }),
  );
}

/** S1: reveals one `mcp.json` in the system file manager. */
async function revealFile(scope: McpScope) {
  const path = filePaths.value[scope];
  if (path === null) return;
  try {
    await revealFileInFileManager(path);
  } catch (cause) {
    fileErrors.value = {
      ...fileErrors.value,
      [scope]: commandErrorMessage(cause),
    };
  }
}

/**
 * S1: ends the live sign-in, if any. Always safe: the session is dropped from the
 * UI first, and Rust treats a stop of an unknown id as `false`.
 */
async function stopLiveSession() {
  const live = session.value;
  session.value = null;
  loginRunning.value = false;
  loginFinished.value = false;
  loginExitCode.value = null;
  // The URL belonged to the child that just went away.
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

/** Hands one URL to the system browser; a rejection is not fatal. */
async function openInBrowser(url: string) {
  openFailed.value = false;
  try {
    await openExternalUrl(url);
  } catch {
    openFailed.value = true;
  }
}

/** The manual fallback behind the address block. */
function openSignInUrl() {
  const url = signInUrl.value;
  if (url === null) return;
  void openInBrowser(url);
}

/**
 * S1: starts `pi mcp login` for one server. Only one sign-in runs at a time, so a
 * second "Sign in" stops the previous flow first — the same rule the accounts
 * block follows.
 */
async function startLogin(server: McpServerSummary) {
  if (starting.value !== null) return;
  const key = rowKey(server);
  starting.value = key;
  error.value = null;
  clearRowError(key);
  try {
    await stopLiveSession();
    loginLines.value = [];
    pastedUrl.value = "";
    loginFinished.value = false;
    loginExitCode.value = null;
    signInUrl.value = null;
    autoOpenedFor.value = null;
    openFailed.value = false;
    detailsOpen.value = false;
    const sessionId = await mcpLoginStart(server.name, server.scope);
    session.value = {
      name: server.name,
      scope: server.scope,
      sessionId,
    };
    loginRunning.value = true;
  } catch (cause) {
    setRowError(key, commandErrorMessage(cause));
  } finally {
    starting.value = null;
  }
}

/**
 * S1: the pasted redirect URL goes to the child's stdin (Rust appends the
 * newline). That is how pi finishes a sign-in whose browser is on another
 * machine, and it is the same seam the device-code login already uses.
 */
async function submitPastedUrl() {
  const live = session.value;
  const value = pastedUrl.value.trim();
  if (!live || value === "" || sendingUrl.value) return;
  const key = rowKey(live);
  sendingUrl.value = true;
  clearRowError(key);
  try {
    const accepted = await piCliWrite(live.sessionId, value);
    if (accepted) {
      pastedUrl.value = "";
    } else {
      setRowError(key, t("settings.mcp.login.notAccepted"));
    }
  } catch (cause) {
    setRowError(key, commandErrorMessage(cause));
  } finally {
    sendingUrl.value = false;
  }
}

/** "Cancel" really kills the child and closes the panel. */
async function cancelLogin() {
  await stopLiveSession();
  loginLines.value = [];
  pastedUrl.value = "";
}

/** Drops the outcome of a sign-in that already ended; the child is gone. */
function dismissLogin() {
  session.value = null;
  loginLines.value = [];
  pastedUrl.value = "";
  loginRunning.value = false;
  loginFinished.value = false;
  loginExitCode.value = null;
  signInUrl.value = null;
  autoOpenedFor.value = null;
  openFailed.value = false;
  detailsOpen.value = false;
}

/** S1: opens the sign-out confirmation for one server. */
function requestSignOut(server: McpServerSummary) {
  signOutTarget.value = server;
  signOutOpen.value = true;
  error.value = null;
}

/** S1: signs out of one server behind a confirmation and adopts the refreshed list. */
async function confirmSignOut() {
  const server = signOutTarget.value;
  if (!server || signingOut.value !== null) return;
  const key = rowKey(server);
  signOutOpen.value = false;
  signOutTarget.value = null;
  signingOut.value = key;
  error.value = null;
  clearRowError(key);
  try {
    // The command answers with the listing taken after the write, so the store
    // adopts it instead of booting `pi mcp list` a second time for the same rows.
    applyMcpServers(await mcpLogout(server.name, server.scope));
  } catch (cause) {
    setRowError(key, commandErrorMessage(cause));
  } finally {
    signingOut.value = null;
  }
}

/** S1: opens the removal confirmation for one server. */
function requestRemove(server: McpServerSummary) {
  removalTarget.value = server;
  removalOpen.value = true;
  error.value = null;
}

/**
 * S1: removes the entry from its file and renders the refreshed list. pi keeps the
 * OAuth credentials of a removed server, so this is not a sign-out and says
 * nothing about one.
 */
async function confirmRemove() {
  const target = removalTarget.value;
  if (!target || busy.value !== null) return;
  removalOpen.value = false;
  removalTarget.value = null;
  busy.value = rowKey(target);
  error.value = null;
  try {
    // A sign-in left running on the removed row would have nothing to report on.
    if (session.value?.name === target.name && session.value.scope === target.scope) {
      await stopLiveSession();
    }
    applyMcpServers(await mcpRemove(target.name, target.scope));
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    busy.value = null;
  }
}

/** S1: a saved server landed in the file, so the answer is the new list. */
function handleSaved(list: McpServerSummary[]) {
  applyMcpServers(list);
  showForm.value = false;
}

/** The streamed events of the one session this pane owns. */
function handleCliEvent(event: PiCliEvent) {
  const live = session.value;
  // Events of an older session (or of another surface) are not ours to render.
  if (!live || event.cliSessionId !== live.sessionId) return;
  if (event.stream === "exit") {
    loginRunning.value = false;
    loginFinished.value = true;
    loginExitCode.value = event.exitCode ?? null;
    // The sign-in is over, so the link is spent; the lines stay as the outcome.
    signInUrl.value = null;
    openFailed.value = false;
    // A successful sign-in leaves one confirmation and nothing to read.
    if (event.exitCode === 0) loginLines.value = [];
    // Rust keeps a session registered until it is stopped, so reap the finished
    // child here instead of leaving an id in the registry for the whole app run.
    void piCliStop(live.sessionId).catch(() => {});
    // The exit is what changed pi's stored credential: ask the file again rather
    // than guessing a badge from the exit code. The rows on screen stay for the
    // whole read.
    void refresh();
    return;
  }
  if (event.line === undefined) return;
  loginLines.value = [...loginLines.value, event.line];
  if (detailsOpen.value) void scrollOutputToEnd();
}

// The sign-in URL is read out of the stream as it arrives and kept for the rest
// of the session; re-reading the whole stream is cheap and keeps no parse state.
watch(
  () => loginLines.value,
  (lines) => {
    if (signInUrl.value !== null) return;
    const found = extractSignInUrl(lines);
    if (found !== null) signInUrl.value = found;
  },
);

// The stream follows its newest line, but only while the human has it open.
watch(detailsOpen, (open) => {
  if (open) void scrollOutputToEnd();
});

// Auto-open, at most once per sign-in session: the guard is the session id, so a
// re-printed URL or a re-render cannot open the browser twice.
watch(signInUrl, (url) => {
  const live = session.value;
  if (url === null || live === null) return;
  if (autoOpenedFor.value === live.sessionId) return;
  autoOpenedFor.value = live.sessionId;
  void openInBrowser(url);
});

onMounted(async () => {
  const unsubscribe = await subscribePiCli(handleCliEvent);
  if (disposed) {
    unsubscribe();
    return;
  }
  unsubscribeEvents = unsubscribe;
});

/**
 * How long the pane waits after mount before reading. Short: the cached rows are
 * already on screen, so this only decides when the background read starts. The
 * idle callback behind it is the accounts block's rule — never compete with a
 * paint or an input frame — with a deadline so a busy webview cannot starve it.
 */
const BACKGROUND_READ_DELAY_MS = 150;
const BACKGROUND_READ_IDLE_TIMEOUT_MS = 600;

/** The scheduled read of this mount, so leaving the pane cancels it. */
let readTimer: ReturnType<typeof setTimeout> | null = null;
let idleHandle: number | null = null;

/** Nothing scheduled may outlive the section that scheduled it. */
function cancelScheduledRead() {
  if (readTimer !== null) {
    clearTimeout(readTimer);
    readTimer = null;
  }
  if (idleHandle !== null) {
    if (typeof cancelIdleCallback === "function") {
      cancelIdleCallback(idleHandle);
    }
    idleHandle = null;
  }
}

/**
 * The background read: an unforced load, so a list the store already holds for
 * this session answers without booting the CLI and a stale one re-reads behind the
 * rows on screen.
 */
function scheduleBackgroundRead() {
  const read = () => {
    readTimer = null;
    if (disposed) return;
    void loadMcpServers();
  };
  readTimer = setTimeout(() => {
    readTimer = null;
    if (disposed) return;
    if (typeof requestIdleCallback === "function") {
      idleHandle = requestIdleCallback(
        () => {
          idleHandle = null;
          if (disposed) return;
          void loadMcpServers();
        },
        { timeout: BACKGROUND_READ_IDLE_TIMEOUT_MS },
      );
      return;
    }
    read();
  }, BACKGROUND_READ_DELAY_MS);
}

// The pane exists only while it is on screen (the dialog renders it with a
// `v-if` chain), so mounting it is exactly "the section was opened". Nothing here
// is awaited: `servers` is painted from whatever the store holds in this frame,
// the read is scheduled behind it, and the two file paths resolve on their own.
onMounted(() => {
  scheduleBackgroundRead();
  void loadFilePaths();
});

onBeforeUnmount(() => {
  disposed = true;
  cancelScheduledRead();
  // pi's `mcp.json` can be edited outside the app, so the next mount reads again
  // instead of trusting rows that may predate the edit.
  invalidateMcpServers();
  unsubscribeEvents?.();
  unsubscribeEvents = null;
  // No sign-in flow outlives the pane that started it.
  void stopLiveSession();
});
</script>

<template>
  <div class="space-y-6 py-4">
    <section class="space-y-2">
      <div class="flex flex-wrap items-start justify-between gap-2">
        <div class="min-w-0">
          <h3 class="text-sm font-medium">{{ t("settings.mcp.title") }}</h3>
          <p class="mt-0.5 text-xs text-muted-foreground">
            {{ t("settings.mcp.description") }}
          </p>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          class="shrink-0"
          :disabled="serversLoading || refreshing"
          @click="refresh"
        >
          {{
            refreshing
              ? t("settings.mcp.refreshing")
              : t("settings.mcp.refresh")
          }}
        </Button>
      </div>

      <!-- A read that failed says so instead of claiming the list is empty: the
           store kept the previous rows, and an unreadable configuration is not
           "nothing configured". It sits here, right under the title, because it
           is the one line that explains why the list below is what it is. -->
      <Alert v-if="serversError" variant="destructive">
        <AlertDescription>{{ serversError }}</AlertDescription>
      </Alert>

      <!-- Two lines, never a row: "still reading" while nobody has answered (the
           store is not settled, holds nothing and nothing failed), and "nothing
           configured" only when an answer genuinely found no server. The rows are
           painted in every other case, including a refresh whose new answer has
           not landed yet. -->
      <p
        v-if="!serversSettled && !servers.length && !serversError"
        class="text-xs text-muted-foreground"
      >
        {{ t("settings.mcp.loading") }}
      </p>

      <template v-else>
        <p
          v-if="!servers.length && !serversError"
          class="text-xs text-muted-foreground"
        >
          {{ t("settings.mcp.empty") }}
        </p>

        <!-- S1: one row per server. The name, the scope badge, the transport, the
             truncated target and the state badge are the summary; the actions are
             siblings, never nested in a button. No rule per row: the pane keeps
             its separators between blocks. -->
        <ul v-if="servers.length" class="space-y-0.5">
          <li
            v-for="server in servers"
            :key="rowKey(server)"
            class="rounded-md px-2 py-1.5 transition-colors hover:bg-muted"
          >
            <div class="flex items-center gap-1">
              <div class="min-w-0 flex-1">
                <div class="flex min-w-0 flex-wrap items-center gap-2">
                  <span class="min-w-0 truncate text-sm font-medium">
                    {{ server.name }}
                  </span>
                  <Badge variant="secondary" class="shrink-0">
                    {{ t(SCOPE_KEYS[server.scope]) }}
                  </Badge>
                  <Badge
                    :variant="badgeOf(server.state).variant"
                    :class="badgeOf(server.state).class"
                    class="shrink-0"
                  >
                    {{ t(badgeOf(server.state).key) }}
                  </Badge>
                  <span
                    v-if="server.tools.length"
                    class="shrink-0 text-xs text-muted-foreground"
                  >
                    {{ t("settings.mcp.tools", { count: server.tools.length }) }}
                  </span>
                </div>
                <div
                  class="mt-0.5 flex min-w-0 items-center gap-2 text-xs text-muted-foreground"
                >
                  <span class="shrink-0 uppercase">
                    {{ server.transport }}
                  </span>
                  <span class="min-w-0 truncate font-mono" :title="server.target">
                    {{ server.target }}
                  </span>
                </div>
              </div>

              <!-- One action per row: the state decides which one; removal is
                   separate because it deletes the entry, not a credential. -->
              <Button
                v-if="offersSignOut(server)"
                type="button"
                variant="ghost"
                size="sm"
                :disabled="signingOut === rowKey(server) || busy !== null"
                @click="requestSignOut(server)"
              >
                {{
                  signingOut === rowKey(server)
                    ? t("settings.mcp.signOutBusy")
                    : t("settings.mcp.signOut")
                }}
              </Button>
              <Button
                v-else
                type="button"
                variant="ghost"
                size="sm"
                :disabled="starting === rowKey(server) || busy !== null"
                @click="startLogin(server)"
              >
                {{
                  starting === rowKey(server)
                    ? t("settings.mcp.signInBusy")
                    : t("settings.mcp.signIn")
                }}
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                :disabled="busy !== null"
                @click="requestRemove(server)"
              >
                {{ t("settings.mcp.remove") }}
              </Button>
            </div>

            <!-- pi's own reason for a server that did not connect. -->
            <p v-if="server.error" class="mt-1 text-xs text-destructive">
              {{ server.error }}
            </p>

            <!-- A rejected action keeps its reason on its own row. -->
            <p
              v-if="rowErrors[rowKey(server)]"
              class="mt-1 text-xs text-destructive"
            >
              {{ rowErrors[rowKey(server)] }}
            </p>

            <!-- S1: the live sign-in, one line by default: what it waits for and
                 the way out. The stream, the URL and the paste field are detail
                 behind a disclosure, exactly like the provider login. -->
            <div
              v-if="
                session?.name === server.name && session.scope === server.scope
              "
              class="mt-2 space-y-1.5"
            >
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

              <div v-if="loginPhase !== 'succeeded'">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  class="h-6 px-1 text-xs font-normal text-muted-foreground"
                  :aria-expanded="detailsOpen"
                  :aria-controls="detailsPanelId(server)"
                  @click="detailsOpen = !detailsOpen"
                >
                  <ChevronRightIcon
                    class="size-3 shrink-0 transition-transform"
                    :class="detailsOpen ? 'rotate-90' : ''"
                    aria-hidden="true"
                  />
                  {{ t("settings.mcp.login.disclosure") }}
                </Button>

                <!-- `v-show` keeps the panel in the document, so `aria-controls`
                     always resolves; a hidden node is not a live region, so
                     nothing is announced before it is opened. -->
                <div
                  v-show="detailsOpen"
                  :id="detailsPanelId(server)"
                  class="mt-1.5 space-y-2"
                >
                  <div
                    :ref="bindOutput"
                    role="log"
                    :aria-label="t('settings.mcp.login.output')"
                    class="max-h-40 overflow-y-auto whitespace-pre-wrap break-words rounded-md border border-border bg-muted/40 p-2 font-mono text-xs text-foreground"
                  >
                    <p
                      v-for="(line, index) in loginLines"
                      :key="index"
                      class="m-0"
                    >
                      {{ line }}
                    </p>
                    <p
                      v-if="!loginLines.length"
                      class="m-0 text-muted-foreground"
                    >
                      {{ t("settings.mcp.login.waitingBrowser") }}
                    </p>
                  </div>

                  <div
                    v-if="signInUrl"
                    class="space-y-1.5 rounded-md border border-primary/40 bg-primary/5 p-2"
                  >
                    <p class="text-xs font-medium">
                      {{ t("settings.mcp.login.urlTitle") }}
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
                      {{ t("settings.mcp.login.openBrowser") }}
                    </Button>
                    <p v-if="openFailed" class="text-xs text-destructive">
                      {{ t("settings.mcp.login.openFailed") }}
                    </p>
                  </div>

                  <p v-if="signInUrl" class="text-xs text-muted-foreground">
                    {{ t("settings.mcp.login.redirectHint") }}
                  </p>

                  <form
                    class="flex flex-wrap items-center gap-2"
                    @submit.prevent="submitPastedUrl"
                  >
                    <Input
                      v-model="pastedUrl"
                      class="h-8 min-w-0 flex-1 font-mono text-sm"
                      autocomplete="off"
                      spellcheck="false"
                      :disabled="!loginRunning || sendingUrl"
                      :aria-label="
                        t('settings.mcp.login.redirectLabel', {
                          name: server.name,
                        })
                      "
                      :placeholder="t('settings.mcp.login.redirectPlaceholder')"
                    />
                    <Button
                      type="submit"
                      size="sm"
                      :disabled="
                        pastedUrl.trim() === '' || sendingUrl || !loginRunning
                      "
                    >
                      {{
                        sendingUrl
                          ? t("settings.mcp.login.sending")
                          : t("settings.mcp.login.continue")
                      }}
                    </Button>
                  </form>
                </div>
              </div>
            </div>
          </li>
        </ul>
      </template>
    </section>

    <Separator />

    <!-- S1: adding a server — the catalogue and the manual form, in one block
         that writes once. It belongs to the list above it, so no rule of its own. -->
    <section class="space-y-3">
      <Button
        v-if="!showForm"
        type="button"
        variant="outline"
        size="sm"
        @click="showForm = true"
      >
        <PlusIcon data-icon="inline-start" />
        {{ t("settings.mcp.add") }}
      </Button>
      <McpServerForm
        v-else
        @saved="handleSaved"
        @cancel="showForm = false"
      />
    </section>

    <Separator />

    <!-- S1: where pi keeps these entries. The path is resolved by Rust; the
         action reveals it in the system file manager (see
         `revealFileInFileManager` for why revealing and not "open with"). -->
    <section class="space-y-2">
      <div class="min-w-0">
        <h3 class="text-sm font-medium">{{ t("settings.mcp.files.title") }}</h3>
        <p class="mt-0.5 text-xs text-muted-foreground">
          {{ t("settings.mcp.files.description") }}
        </p>
      </div>

      <div
        v-for="scope in SCOPES"
        :key="`file-${scope}`"
        class="flex flex-wrap items-center gap-2"
      >
        <span class="w-14 shrink-0 text-xs font-medium">
          {{ t(SCOPE_KEYS[scope]) }}
        </span>
        <span
          class="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground"
          :title="filePaths[scope] ?? ''"
        >
          {{ filePaths[scope] ?? t("settings.mcp.files.unknown") }}
        </span>
        <Button
          type="button"
          variant="outline"
          size="sm"
          class="shrink-0"
          :disabled="filePaths[scope] === null"
          @click="revealFile(scope)"
        >
          <ExternalLinkIcon data-icon="inline-start" />
          {{ t("settings.mcp.files.show") }}
        </Button>
        <p v-if="fileErrors[scope]" class="w-full text-xs text-destructive">
          {{ fileErrors[scope] }}
        </p>
      </div>
    </section>

    <!-- S1: one confirmation for the whole list, like the provider removal: the
         row only says which server it is about. -->
    <Dialog v-model:open="removalOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{{ t("settings.mcp.remove.title") }}</DialogTitle>
          <DialogDescription>
            {{
              t("settings.mcp.remove.description", {
                name: removalTarget?.name ?? "",
                scope: removalTarget ? t(SCOPE_KEYS[removalTarget.scope]) : "",
              })
            }}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <DialogClose as-child>
            <Button variant="outline">{{ t("common.cancel") }}</Button>
          </DialogClose>
          <Button
            variant="destructive"
            :disabled="busy !== null"
            @click="confirmRemove"
          >
            {{ t("settings.mcp.remove") }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- S1: the sign-out confirmation, one dialog for the whole list. -->
    <Dialog v-model:open="signOutOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{{ t("settings.mcp.signOut.title") }}</DialogTitle>
          <DialogDescription>
            {{
              t("settings.mcp.signOut.description", {
                name: signOutTarget?.name ?? "",
              })
            }}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <DialogClose as-child>
            <Button variant="outline">{{ t("common.cancel") }}</Button>
          </DialogClose>
          <Button
            variant="destructive"
            :disabled="signingOut !== null"
            @click="confirmSignOut"
          >
            {{ t("settings.mcp.signOut") }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- The pane's write error line: an add, remove, sign-in or sign-out that
         could not run at all. A failed read is the store's (`serversError`, under
         the title above) and a server's own failure is pi's, on its row. -->
    <Alert v-if="error" variant="destructive">
      <AlertDescription>{{ error }}</AlertDescription>
    </Alert>
  </div>
</template>
