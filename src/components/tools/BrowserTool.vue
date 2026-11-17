<script setup lang="ts">
// Browser tab (F3 of `odd/tasks/tool-drawer-and-mcp.md`): Nabla LANDS a dedicated
// Chromium for the workspace and then only reports what it has open.
//
// WHO NAVIGATES — the agent (S4). This tab is not a browser: it has no address
// bar of its own driving a viewport, no back button and no content. The agent
// reaches the instance through CDP (`pi-browser-harness`) or through the
// `chrome-devtools-mcp` server configured in Settings → MCP, and this pane is the
// human's window into that: which browser runs, on which port, and what pages are
// open. The one action it offers — Open URL — is here so the human can steer
// without a second terminal; it is still the SAME session the agent drives.
//
// WHY LAUNCH AND NOT ATTACH (F3 decision) — a dedicated instance with its own
// profile and its own `--remote-debugging-port` is reproducible and never touches
// the human's cookies and logins. Attaching to an already-open Chrome would
// require them to relaunch it with a debug flag and would hand the agent their
// live session, so it is out of scope.
//
// WHAT IS SUPPORTED (S6) — only Chromium speaks CDP. `browser_detect` therefore
// answers an EMPTY list on a machine with only Safari or Firefox, and this pane
// turns that into a disabled face with the reason and one way out: the
// `chrome-devtools-mcp` server from Settings → MCP. It never renders a Launch
// button that cannot work.
//
// POLLING — targets are read from `browser_targets` every few seconds, and only
// while there is something to read AND the human could see the answer: a hidden
// document or a blurred window STOPS the interval (the requirement is literal,
// not "skip a tick"), and regaining visibility or focus refreshes once and
// restarts it. Coming back therefore shows current state instead of replaying the
// missed ticks. Requests are serialized (`targetsInFlight`) and answered only if
// the session that asked is still the one mounted.
//
// TEARDOWN — one session per mounting. Unmount closes it (workspace changes
// remount the tab, see `ToolTabs`), a workspace change while mounted closes it
// too, and a launch that answers after teardown closes the browser it just
// created instead of leaking it. Nothing keeps polling afterwards.
import { computed, onBeforeUnmount, onMounted, ref, watch, type Component } from "vue";
import {
  CircleIcon,
  CpuIcon,
  FrameIcon,
  GlobeIcon,
  PlayIcon,
  SquareIcon,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  browserClose,
  browserDetect,
  browserLaunch,
  browserOpenUrl,
  browserTargets,
  commandErrorMessage,
} from "@/contracts/commands";
import type {
  BrowserCandidate,
  BrowserSession,
  BrowserTarget,
  WorkspaceInfo,
} from "@/contracts/types";
import { t, type MessageKey } from "@/lib/i18n";

const props = defineProps<{
  workspace: WorkspaceInfo | null;
}>();

/**
 * How often the open targets are re-read while the pane is watched. Three seconds
 * is fast enough to watch the agent work and slow enough that the CDP query never
 * becomes the reason the drawer feels busy.
 */
const POLL_MS = 3_000;

/**
 * Icon and label per target kind. Indexed as a plain record (not a union-keyed
 * one) because the BROWSER is the authority on `kind`: Rust forwards the DevTools
 * `type` lowercased, and a type this build has never seen must render as "Other"
 * instead of emptying the list. `service_worker` and `background_page` fold into
 * the worker label: they are the same thing to a reader.
 */
const KIND_META: Record<string, { icon: Component; labelKey: MessageKey }> = {
  page: { icon: GlobeIcon, labelKey: "tools.browser.kind.page" },
  iframe: { icon: FrameIcon, labelKey: "tools.browser.kind.iframe" },
  worker: { icon: CpuIcon, labelKey: "tools.browser.kind.worker" },
  service_worker: { icon: CpuIcon, labelKey: "tools.browser.kind.worker" },
  background_page: { icon: CpuIcon, labelKey: "tools.browser.kind.worker" },
  other: { icon: CircleIcon, labelKey: "tools.browser.kind.other" },
};
const FALLBACK_KIND = KIND_META.other;

function kindMeta(kind: string): { icon: Component; labelKey: MessageKey } {
  return KIND_META[kind] ?? FALLBACK_KIND;
}

/** Detection. */
const detecting = ref(true);
const detectError = ref<string | null>(null);
const candidates = ref<BrowserCandidate[]>([]);

/** The session this tab launched, and nothing else: `null` means not launched. */
const session = ref<BrowserSession | null>(null);
const launching = ref(false);
const closing = ref(false);
/**
 * One localized sentence for whatever the last launch, close or open attempt
 * failed with. The targets keep their own error so a failed poll does not erase
 * the status row above it.
 */
const actionError = ref<string | null>(null);

/** What the session has open. */
const targets = ref<BrowserTarget[]>([]);
const targetsError = ref<string | null>(null);

/** The Open URL draft. */
const urlDraft = ref("");
const opening = ref(false);

/** Set on unmount: a late answer must never touch a torn-down tab. */
let disposed = false;
let pollHandle: number | null = null;
/** Sequence guard for `browser_targets` answers. */
let targetRequestSeq = 0;
/** One targets request at a time: a poll never piles onto a request in flight. */
let targetsInFlight = false;

/** The browser the Launch button would spawn: the first one `browser_detect` found. */
const primaryName = computed(() => candidates.value[0]?.name ?? "");

/**
 * The draft URL, or `null` when it is empty or not something this pane may open.
 * Only `http:`/`https:` pass: `file:`, `javascript:` and friends would navigate
 * the agent's browser somewhere a URL field has no business going.
 */
const parsedUrl = computed<URL | null>(() => {
  const raw = urlDraft.value.trim();
  if (raw === "") return null;
  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    return null;
  }
  return parsed.protocol === "http:" || parsed.protocol === "https:" ? parsed : null;
});

/** Non-empty and unusable: the only case where the scheme hint is worth showing. */
const urlInvalid = computed(
  () => urlDraft.value.trim() !== "" && parsedUrl.value === null,
);

const canOpen = computed(
  () => session.value !== null && parsedUrl.value !== null && !opening.value,
);

/**
 * The one line each row shows: its title, else its url, else its kind. The wire
 * carries EMPTY strings rather than nulls (Rust's `field_text`), and a page that
 * has not loaded yet reports no title — an empty row would look like a bug.
 */
function targetText(target: BrowserTarget): string {
  const title = target.title.trim();
  if (title !== "") return title;
  const url = target.url.trim();
  if (url !== "") return url;
  return t(kindMeta(target.kind).labelKey);
}

/** The full text on hover, so a truncated row is still readable. */
function targetHint(target: BrowserTarget): string {
  const kind = t(kindMeta(target.kind).labelKey);
  const url = target.url.trim();
  return url === "" ? kind : `${kind} · ${url}`;
}

/** Visible and focused: the two conditions the poll is allowed to run under. */
function mayReadTargets(): boolean {
  if (typeof document === "undefined") return false;
  return document.visibilityState === "visible" && document.hasFocus();
}

function stopPolling(): void {
  if (pollHandle !== null) {
    window.clearInterval(pollHandle);
    pollHandle = null;
  }
}

function startPolling(): void {
  stopPolling();
  if (session.value === null || !mayReadTargets()) return;
  pollHandle = window.setInterval(() => {
    void refreshTargets();
  }, POLL_MS);
}

/**
 * The single listener for visibility, focus and blur: hidden or unfocused stops
 * the interval, coming back refreshes once immediately and restarts it.
 */
function syncPolling(): void {
  if (session.value === null || !mayReadTargets()) {
    stopPolling();
    return;
  }
  void refreshTargets();
  startPolling();
}

/** Reads the open targets once, superseding nothing and answering only if current. */
async function refreshTargets(): Promise<void> {
  const current = session.value;
  if (current === null || targetsInFlight) return;
  const id = (targetRequestSeq += 1);
  targetsInFlight = true;
  try {
    const answer = await browserTargets(current.sessionId);
    if (disposed || id !== targetRequestSeq) return;
    // A different session (or none) owns the tab now: its list is not this one.
    if (session.value?.sessionId !== current.sessionId) return;
    targets.value = answer;
    targetsError.value = null;
  } catch (cause) {
    if (disposed || id !== targetRequestSeq) return;
    if (session.value?.sessionId !== current.sessionId) return;
    targetsError.value = commandErrorMessage(cause);
  } finally {
    if (id === targetRequestSeq) targetsInFlight = false;
  }
}

/** S6: what this machine can run. An empty answer is the disabled face. */
async function detect(): Promise<void> {
  detecting.value = true;
  detectError.value = null;
  try {
    const found = await browserDetect();
    if (disposed) return;
    candidates.value = found;
  } catch (cause) {
    if (disposed) return;
    candidates.value = [];
    detectError.value = commandErrorMessage(cause);
  } finally {
    if (!disposed) detecting.value = false;
  }
}

/** Launches the dedicated instance for this workspace and starts reporting it. */
async function launch(): Promise<void> {
  const workspace = props.workspace;
  if (workspace === null || launching.value || session.value !== null) return;
  actionError.value = null;
  launching.value = true;
  try {
    const opened = await browserLaunch(workspace.path);
    if (disposed) {
      // The tab closed while the browser was starting: do not leak the process.
      void browserClose(opened.sessionId).catch(() => {});
      return;
    }
    session.value = opened;
    targets.value = [];
    targetsError.value = null;
    startPolling();
    await refreshTargets();
  } catch (cause) {
    if (disposed) return;
    actionError.value = t("tools.browser.launchError", {
      message: commandErrorMessage(cause),
    });
  } finally {
    if (!disposed) launching.value = false;
  }
}

/**
 * Ends the session. Idempotent and safe to call from teardown: the ref is cleared
 * first, so a second call has nothing to close.
 */
async function closeSession(): Promise<void> {
  const open = session.value;
  session.value = null;
  targets.value = [];
  targetsError.value = null;
  targetsInFlight = false;
  targetRequestSeq += 1;
  stopPolling();
  if (open === null) return;
  closing.value = true;
  try {
    await browserClose(open.sessionId);
  } catch (cause) {
    if (disposed) return;
    actionError.value = t("tools.browser.closeError", {
      message: commandErrorMessage(cause),
    });
  } finally {
    if (!disposed) closing.value = false;
  }
}

/** S4: hands a validated URL to the session and re-reads what is open. */
async function openUrl(): Promise<void> {
  const current = session.value;
  const parsed = parsedUrl.value;
  if (current === null || parsed === null || opening.value) return;
  actionError.value = null;
  opening.value = true;
  try {
    await browserOpenUrl(current.sessionId, parsed.toString());
    if (disposed) return;
    // The URL takes effect in the browser; the field has nothing left to tell.
    urlDraft.value = "";
    await refreshTargets();
  } catch (cause) {
    if (disposed) return;
    actionError.value = t("tools.browser.openError", {
      message: commandErrorMessage(cause),
    });
  } finally {
    if (!disposed) opening.value = false;
  }
}

// A workspace switch remounts this tab (`ToolTabs` keys it by workspace); the
// watch is the explicit second path, so the browser never outlives the folder it
// was launched for even if the host ever stops remounting.
watch(
  () => props.workspace?.path ?? null,
  (next, previous) => {
    if (next === previous || session.value === null) return;
    void closeSession();
  },
);

onMounted(() => {
  void detect();
  document.addEventListener("visibilitychange", syncPolling);
  window.addEventListener("focus", syncPolling);
  window.addEventListener("blur", syncPolling);
});

onBeforeUnmount(() => {
  disposed = true;
  document.removeEventListener("visibilitychange", syncPolling);
  window.removeEventListener("focus", syncPolling);
  window.removeEventListener("blur", syncPolling);
  stopPolling();
  targetRequestSeq += 1;
  targetsInFlight = false;
  const open = session.value;
  session.value = null;
  targets.value = [];
  if (open !== null) {
    // Best effort: `browser_close` is idempotent and the process must not outlive
    // the tab that launched it.
    void browserClose(open.sessionId).catch(() => {});
  }
});
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <p v-if="detecting" class="p-3 text-xs text-muted-foreground">
      {{ t("tools.browser.detecting") }}
    </p>

    <!-- Detection itself failed (a permissions or spawn error, not "none found"):
         the unsupported face would be a lie here, so say what happened. -->
    <div v-else-if="detectError" class="flex flex-col items-start gap-2 p-3">
      <p class="text-xs text-destructive">
        {{ t("tools.browser.detectError", { message: detectError }) }}
      </p>
      <Button variant="outline" size="xs" @click="detect">
        {{ t("tools.browser.retry") }}
      </Button>
    </div>

    <!-- S6: the disabled face. A reason, and the one supported way around it. -->
    <div v-else-if="candidates.length === 0" class="flex flex-1 flex-col items-start gap-2 p-3">
      <p class="text-xs text-muted-foreground">
        {{ t("tools.browser.unsupported") }}
      </p>
      <p class="text-xs text-muted-foreground">
        {{ t("tools.browser.mcpPointer") }}
      </p>
    </div>

    <p
      v-else-if="workspace === null"
      class="flex flex-1 items-center justify-center p-4 text-center text-xs text-muted-foreground"
    >
      {{ t("tools.browser.noWorkspace") }}
    </p>

    <template v-else>
      <!-- Not launched: one button naming the browser it will spawn. -->
      <div
        v-if="session === null"
        class="flex shrink-0 flex-col items-start gap-1.5 px-2 py-2"
      >
        <Button size="xs" :disabled="launching" @click="launch">
          <PlayIcon data-icon="inline-start" aria-hidden="true" />
          {{
            launching
              ? t("tools.browser.launching")
              : t("tools.browser.launch", { name: primaryName })
          }}
        </Button>
        <p
          v-if="candidates.length > 1"
          class="text-xs text-muted-foreground"
        >
          {{ t("tools.browser.multiple", { name: primaryName }) }}
        </p>
      </div>

      <!-- Launched: what runs, on which port, and the one way to end it. -->
      <div
        v-else
        class="flex shrink-0 items-center gap-2 border-b px-2 py-1.5"
      >
        <span
          class="min-w-0 flex-1 truncate text-xs"
          :title="t('tools.browser.profile', { path: session.profileDir })"
        >
          {{
            t("tools.browser.running", {
              name: session.browserName,
              port: session.port,
            })
          }}
        </span>
        <Button
          variant="outline"
          size="xs"
          class="shrink-0"
          :disabled="closing"
          @click="closeSession"
        >
          <SquareIcon data-icon="inline-start" aria-hidden="true" />
          {{ t("tools.browser.close") }}
        </Button>
      </div>

      <p
        v-if="actionError"
        class="shrink-0 border-b px-2 py-1 text-xs text-destructive"
      >
        {{ actionError }}
      </p>

      <template v-if="session !== null">
        <!-- Open URL: http(s) only, and the button says so by staying disabled. -->
        <form
          class="flex shrink-0 items-center gap-1.5 px-2 pt-1.5 pb-1"
          @submit.prevent="openUrl"
        >
          <Input
            v-model="urlDraft"
            class="h-7 text-xs"
            type="text"
            inputmode="url"
            autocomplete="off"
            spellcheck="false"
            :placeholder="t('tools.browser.openUrlPlaceholder')"
            :aria-label="t('tools.browser.openUrl')"
            :aria-invalid="urlInvalid"
            :disabled="opening"
          />
          <Button type="submit" size="xs" class="shrink-0" :disabled="!canOpen">
            {{ opening ? t("tools.browser.opening") : t("tools.browser.open") }}
          </Button>
        </form>

        <p v-if="urlInvalid" class="shrink-0 px-2 pb-1 text-xs text-warning">
          {{ t("tools.browser.urlInvalid") }}
        </p>

        <p
          v-if="targetsError"
          class="shrink-0 px-2 pb-1 text-xs text-destructive"
        >
          {{ t("tools.browser.targetsError", { message: targetsError }) }}
        </p>

        <!-- Read-only status: rows are not buttons, and clicking one does nothing. -->
        <div class="min-h-0 flex-1 overflow-auto pb-1">
          <p
            class="px-2 pt-1 pb-1 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase"
          >
            {{ t("tools.browser.targets") }}
          </p>

          <p
            v-if="targets.length === 0 && targetsError === null"
            class="px-2 py-1 text-xs text-muted-foreground"
          >
            {{ t("tools.browser.targetsEmpty") }}
          </p>

          <div
            v-for="target in targets"
            :key="target.id"
            class="flex min-w-0 items-center gap-2 px-2 py-1 text-xs"
            :title="targetHint(target)"
          >
            <component
              :is="kindMeta(target.kind).icon"
              class="size-3.5 shrink-0 text-muted-foreground"
              aria-hidden="true"
            />
            <span class="min-w-0 flex-1 truncate">{{ targetText(target) }}</span>
            <span class="sr-only">{{ t(kindMeta(target.kind).labelKey) }}</span>
          </div>
        </div>
      </template>

      <!-- The honest half of S4: this pane shows, the agent drives. -->
      <p
        class="mt-auto shrink-0 border-t px-2 py-1.5 text-xs text-muted-foreground"
      >
        {{ t("tools.browser.agentNote") }}
      </p>
    </template>
  </div>
</template>
