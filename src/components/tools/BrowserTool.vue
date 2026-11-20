<script setup lang="ts">
// Browser pane: a real webview the APP owns, drawn as a child webview of the
// main window (`src-tauri/src/embed.rs`).
//
// WHY A CHILD WEBVIEW — an iframe cannot load the web: `X-Frame-Options` and CSP
// `frame-ancestors` refuse to be framed, so a pane built on one would be blank on
// most sites. An external Chromium (the previous shape of this tool) is a
// different product: it leaves the app window and the pane could only report it.
// A Tauri child webview keeps one window, one app UI, and one page the human
// reads while the session runs.
//
// THE PANE MUST RESERVE THE RECT — a child webview floats ABOVE the HTML. It is
// not a DOM node, it cannot be styled, clipped or scrolled by CSS, and anything
// the pane draws in its area is invisible. So the pane keeps two regions: a
// toolbar (address bar, Back/Forward/Reload, status) that is real HTML, and a
// BODY it leaves empty, whose measured `getBoundingClientRect()` is exactly what
// the webview is created with and moved to. `x`/`y`/`width`/`height` are LOGICAL
// pixels all the way through: CSS pixels in this webview are logical pixels, and
// Tauri's `LogicalPosition`/`LogicalSize` take them unchanged.
//
// KEEPING IT IN SYNC — the body rect changes for reasons the pane does not
// control: the drawer is dragged wider, the dock is re-split, the window is
// resized, an ancestor scrolls, a sidebar transition animates. A ResizeObserver
// on the body and on the pane root covers size changes; `resize` and a CAPTURE
// `scroll` listener cover position-only and scrolling changes (`scroll` does not
// bubble). All of them schedule ONE `embed_set_bounds` per animation FRAME, so a
// drag or a live window resize asks Rust once per frame instead of once per event
// burst, and the last frame wins. When the layout stops moving, one SETTLE pass
// places the view once more whether or not the rect changed: the bridge call is
// fire-and-forget (see `embed.rs`), so a placement the platform never applied is
// otherwise never retried, and the native view keeps the old rect while the pane
// believes it moved the view.
//
// WHO DRIVES IT — the app does, and only the app: Back, Forward, Reload (through
// `embed_eval`) and the address bar (through `embed_navigate`). An agent cannot
// reach this webview yet; that arrives with a local MCP. The pane says so in the
// footer rather than implying a capability that does not exist.
//
// TEARDOWN — a view lives as long as this pane is DRAWN, and the two reasons it
// stops being drawn are not the same thing. The drawer keeps every tab's body
// mounted and only shows the active one (`v-show` in `ToolTabs.vue`) so a terminal
// keeps its shell; a child webview cannot be hidden that way, because it floats
// above the HTML and would keep painting over whatever tab is on screen. So the
// pane receives `active` (its tab is drawn) and `visible` (the drawer is on
// screen):
//
//   - tab not drawn (`active === false`): the view is CLOSED when the tab leaves
//     the screen and rebuilt when it comes back. A tab switch is the human saying
//     they are looking at something else, and a fresh view for each tab is what
//     keeps four tabs from holding four live pages;
//   - drawer collapsed (`visible === false`): the view is HIDDEN, never closed.
//     `embed_set_visible` is the platform's own switch, so the drawer reopening
//     restores the SAME page — document, history, scroll position and login — over
//     a re-measured rect. Closing it here is exactly the bug this pane used to
//     carry one level up: collapsing the drawer restarted every tool.
//
// Everything else follows the component: unmount closes the view (closing the pane,
// quitting), a workspace switch closes it and rebuilds it, and a create that
// answers after teardown closes the view it just made instead of leaking it.
//
// URL POLICY — `http`, `https` and `about:blank`, nothing else, and the check is
// LOCAL: a rejected address shows the message and never reaches Rust. A bare host
// (`example.com`, `localhost:1420`) gains `https://`, which is what a browser's
// address bar does; `file:`, `javascript:`, `data:` and every other scheme is
// refused with the same message.
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  RotateCwIcon,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  commandErrorMessage,
  embedClose,
  embedCreate,
  embedEval,
  embedNavigate,
  embedSetBounds,
  embedSetVisible,
  embedUrl,
  subscribeEmbedNav,
} from "@/contracts/commands";
import type { EmbedNavEvent, WorkspaceInfo } from "@/contracts/types";
import { t } from "@/lib/i18n";

const props = defineProps<{
  workspace: WorkspaceInfo | null;
  /**
   * This pane's tab is the drawn one. `false` closes the child webview (it cannot
   * be hidden) and `true` builds it again; absent means "drawn", so a pane that
   * does not know about tabs still works.
   */
  active?: boolean;
  /**
   * The drawer this pane lives in is on screen. `false` HIDES the child webview
   * natively (CSS cannot) and `true` shows it again with the same page loaded;
   * absent means "on screen".
   */
  visible?: boolean;
}>();

/** Whether this pane should have a live view right now (see the teardown note). */
const drawn = computed(() => props.active !== false);

/**
 * Whether the drawer is on screen, which is the difference between hiding a view
 * and closing it (see the teardown note).
 */
const shown = computed(() => props.visible !== false);

/** What a fresh view shows. The pane offers no generated start page: only the
 *  http(s) addresses Rust accepts, and a blank document until the human types. */
const START_URL = "about:blank";

/**
 * The fallback window when this webview has no `requestAnimationFrame`: one update
 * per ~30 ms. It is a fallback, never the normal path — a coalescer keyed on a
 * timer is exactly the "one update per event burst" shape that leaves a live
 * resize on a stale rect.
 */
const BOUNDS_FALLBACK_MS = 30;

/**
 * How long the layout must stay still before the settle pass runs: long enough for
 * the last frame of a drag or a window resize, short enough that the human never
 * sees the view on the previous rect.
 */
const SETTLE_MS = 160;

/**
 * A fallback that stays inside the url policy: the authority a human types
 * without a scheme (`example.com`, `example.com:8080`, `[::1]:1420`) is accepted
 * and gets `https://`, while `javascript:alert(1)`, `data:…`, `file:///…` and
 * `mailto:…` fail this test and are refused with the message below instead of
 * being turned into a nonsense host.
 */
const IMPLICIT_HOST = /^(\[[0-9a-fA-F:.]+\]|[A-Za-z0-9][A-Za-z0-9.-]*)(:\d{1,5})?$/;

/** `null` for anything that is not `http`, `https` or `about:blank`. */
function parseHttp(raw: string): string | null {
  try {
    const parsed = new URL(raw);
    return parsed.protocol === "http:" || parsed.protocol === "https:"
      ? parsed.toString()
      : null;
  } catch {
    return null;
  }
}

/**
 * The url to hand Rust for what the human typed, or `null` when it is not a
 * navigation at all: empty, or a scheme this pane may not open.
 */
function normalizeAddress(raw: string): string | null {
  const trimmed = raw.trim();
  if (trimmed === "") return null;
  if (trimmed.toLowerCase() === START_URL) return START_URL;
  const direct = parseHttp(trimmed);
  if (direct !== null) return direct;
  // An explicit scheme that did not parse as http(s) is a refusal, never a host.
  if (trimmed.includes("://")) return null;
  const authority = trimmed.split("/")[0] ?? "";
  if (!IMPLICIT_HOST.test(authority)) return null;
  return parseHttp(`https://${trimmed}`);
}

/** The two nodes whose geometry the webview follows. */
const rootEl = ref<HTMLElement | null>(null);
const bodyEl = ref<HTMLElement | null>(null);

/** The live child webview, or `null` while it is being built (or after a failure). */
const viewId = ref<string | null>(null);
const creating = ref(false);
const navigating = ref(false);

/**
 * What the address bar holds while the human edits it. Kept apart from
 * `currentUrl` so a navigation the page starts on its own (`embed-nav`) never
 * overwrites what is being typed; the field re-syncs when it loses focus.
 */
const draft = ref("");
/** What the page is actually showing, from `embed-nav` and `embed_url`. */
const currentUrl = ref(START_URL);
const addressFocused = ref(false);

/** One localized sentence for the last failure, shown in the status row. */
const status = ref<string | null>(null);

/** The address bar is empty: the hint, not an error, is what it deserves. */
const draftEmpty = computed(() => draft.value.trim() === "");

/** Non-empty and unusable: the one case where the scheme message belongs. */
const addressInvalid = computed(
  () => !draftEmpty.value && normalizeAddress(draft.value) === null,
);

/** Go is offered only when there is somewhere to go and a view to go there in. */
const canGo = computed(
  () =>
    viewId.value !== null &&
    !creating.value &&
    !navigating.value &&
    !draftEmpty.value,
);

/** The view has not been sent anywhere yet: the toolbar shows the hint. */
const showingBlank = computed(
  () => currentUrl.value === "" || currentUrl.value === START_URL,
);

/** Set on unmount: a late answer must never touch a torn-down pane. */
let disposed = false;

/** The `embed-nav` unsubscribe, once the listener is up. */
let unsubscribeNav: (() => void) | null = null;

/**
 * One rect of this pane, in viewport pixels (this webview's CSS pixels ARE the
 * logical pixels the platform places the child webview in, so no conversion is
 * involved: `embed.rs` normalizes it to whole units once).
 */
interface PaneRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** The pending coalesced bounds update (one per animation frame), if any. */
let frameHandle: number | null = null;
/** Whether `frameHandle` came from `setTimeout` (this webview has no `rAF`). */
let frameIsTimer = false;
/** One `embed_set_bounds` at a time; a request during flight re-runs afterwards. */
let boundsInFlight = false;
let boundsAgain = false;
/** The pending settle pass, if any (see `scheduleSettle`). */
let settleTimer: number | null = null;

/**
 * The rect this pane HANDED to the bridge; `null` before the first one and after
 * every teardown. Deliberately not called "applied": `embed_set_bounds` is
 * fire-and-forget, so this is what was asked for, not what the platform did.
 */
let sentRect: PaneRect | null = null;

let resizeObserver: ResizeObserver | null = null;

/**
 * How long a layout TRANSITION keeps the rect sampled. A transition animates the
 * body rect without resizing anything the observers can see (the sidebar
 * collapsing shifts this pane sideways at a constant width), and it does report
 * `transitionrun`/`transitionend`, so the pane samples for a little longer than
 * any of them runs instead of polling forever.
 */
const TRANSITION_BURST_MS = 500;
/**
 * The only transitions that can move or resize this pane. A button fading its
 * background is a `transitionrun` too, and sampling the rect for half a second
 * every time the pointer crosses a row would be pure noise.
 */
const GEOMETRY_PROPERTY =
  /(^|\-)(width|height|top|right|bottom|left|inset|margin|padding|size|translate|transform|flex|grid|gap)/;
/** A transition is running: its end, in `performance.now()` milliseconds. */
let burstEnds = 0;
let burstTimer: number | null = null;

function nextFrame(): Promise<void> {
  return new Promise((resolve) => {
    if (typeof requestAnimationFrame === "undefined") {
      resolve();
      return;
    }
    requestAnimationFrame(() => resolve());
  });
}

/**
 * The body rect in logical pixels, rounded to whole units, so a sub-pixel wobble
 * does not count as a move and the "nothing changed" check stays meaningful.
 * `null` when there is nothing to cover: unmounted, or the pane is collapsed to
 * zero.
 */
function bodyRect(): PaneRect | null {
  const element = bodyEl.value;
  if (element === null) return null;
  const rect = element.getBoundingClientRect();
  const width = Math.round(rect.width);
  const height = Math.round(rect.height);
  if (width < 1 || height < 1) return null;
  return {
    x: Math.round(rect.left),
    y: Math.round(rect.top),
    width,
    height,
  };
}

/**
 * The platform answer that means the id no longer names a live webview.
 *
 * `EmbedError::UnknownView` renders as `embed: no embedded browser <id> is open`,
 * and both `embed_set_bounds` and `embed_set_visible` answer exactly that when the
 * registry entry is gone or the webview vanished under it.
 *
 * The match is deliberately NARROW — `no embedded browser`, not `embed:` and not
 * every rejection — because a placement failure, a platform error or a refused
 * call is a different thing that belongs in the status row: rebuilding the view
 * there would throw away the page the human was reading to hide a problem that has
 * nothing to do with the id.
 *
 * The honest alternative is a typed rejection across the bridge — a `kind` field
 * on the error the pane could switch on — which would make a substring match
 * unnecessary. Until the contract carries one, the message is the only signal
 * available here, so the narrowest string that identifies this case is the one used.
 */
const DEAD_VIEW_MESSAGE = "no embedded browser";

/** Whether one bridge failure says the view this pane holds is gone. */
function isDeadView(cause: unknown): boolean {
  return commandErrorMessage(cause).includes(DEAD_VIEW_MESSAGE);
}

/**
 * Drops a view the platform says is gone, and rebuilds it once if the pane is drawn.
 *
 * Clearing `viewId` is what heals the pane, and it heals three things at once: the
 * placeholder (`v-if="viewId === null && !creating"`) becomes visible again, so the
 * body stops reserving gray space for a webview nobody paints; the bounds loop
 * stops, because `syncBounds` returns early on a null id and the pending frame
 * then finds nothing to place; and the toolbar controls go back to disabled instead
 * of calling into a dead id.
 *
 * The rebuild is ONE attempt through the existing `createView`, never a retry loop:
 * `createView` bails out while a view exists or while one is being built, so a
 * second failure lands in its own `catch` — the placeholder and the creation error,
 * which is the honest state. A pane that is not drawn, or whose drawer is collapsed,
 * has no view by design (see the teardown note), so nothing is rebuilt there and the
 * watcher that owns that state builds it when it comes back.
 *
 * The new view starts at `START_URL`, because the page went away with the webview:
 * there is nothing left to restore, and pretending otherwise would mean a second
 * creation path carrying a url this pane can no longer verify.
 */
function healDeadView(): void {
  if (viewId.value === null) return;
  viewId.value = null;
  // Nothing is known about the view that replaces this one.
  sentRect = null;
  status.value = null;
  if (disposed || !drawn.value || !shown.value) return;
  void createView();
}

/**
 * Applies the rect now, serialized: a second request while one is in flight is
 * remembered and re-run afterwards rather than stacked, so a drag cannot leave the
 * webview on a stale rect because the newest call resolved first.
 */
async function syncBounds(): Promise<void> {
  const id = viewId.value;
  if (id === null || disposed) return;
  if (boundsInFlight) {
    boundsAgain = true;
    return;
  }
  const rect = bodyRect();
  // Collapsed: nothing to cover. The next resize brings a rect back.
  if (rect === null) return;
  // Already asked for: a colour transition or a scroll that moved nothing must
  // not cost a bridge call. The settle pass clears this once, so the LAST rect of
  // a gesture is always placed again even if this shortcut would have skipped it.
  if (sentRect !== null && sameRect(sentRect, rect)) return;
  boundsInFlight = true;
  try {
    await embedSetBounds(id, rect.x, rect.y, rect.width, rect.height);
    sentRect = rect;
  } catch (cause) {
    if (!disposed) {
      // A view that is gone is not a layout failure: healing it is what stops the
      // pane repeating this error over an empty rect forever.
      if (isDeadView(cause)) {
        healDeadView();
      } else {
        status.value = t("tools.browser.layoutError", {
          message: commandErrorMessage(cause),
        });
      }
    }
  } finally {
    boundsInFlight = false;
    if (boundsAgain) {
      boundsAgain = false;
      scheduleBounds();
    }
  }
}

/** Two rects describe the same rectangle, for the "nothing moved" shortcut. */
function sameRect(a: PaneRect, b: PaneRect): boolean {
  return (
    a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height
  );
}

/**
 * Coalesces every layout signal into ONE update per animation frame, and arms the
 * settle pass behind it.
 *
 * Per FRAME and not per event burst: a drag and a live window resize emit several
 * events per frame, and a timer-keyed coalescer (the shape this pane had) drops
 * every event that arrives while its timer runs — which is how the last frame of a
 * resize reached the platform only if the timing happened to line up.
 */
function scheduleBounds(): void {
  if (disposed) return;
  scheduleFrame();
  scheduleSettle();
}

/** Runs `syncBounds` on the next frame, at most one queued frame at a time. */
function scheduleFrame(): void {
  if (disposed || frameHandle !== null) return;
  if (typeof requestAnimationFrame === "undefined") {
    frameIsTimer = true;
    frameHandle = window.setTimeout(runFrame, BOUNDS_FALLBACK_MS);
    return;
  }
  frameIsTimer = false;
  frameHandle = requestAnimationFrame(runFrame);
}

function runFrame(): void {
  frameHandle = null;
  void syncBounds();
}

/** Drops a queued frame; the next signal starts a new one. */
function cancelFrame(): void {
  if (frameHandle === null) return;
  if (frameIsTimer) {
    window.clearTimeout(frameHandle);
  } else if (typeof cancelAnimationFrame !== "undefined") {
    cancelAnimationFrame(frameHandle);
  }
  frameHandle = null;
}

/**
 * The settle pass: one UNCONDITIONAL placement once the layout stops moving.
 *
 * `embed_set_bounds` is fire-and-forget across the bridge (`src-tauri/src/embed.rs`):
 * the command answers as soon as the placement is queued, and the platform's own
 * failures are logged inside the framework, never returned. So a placement that did
 * not land is indistinguishable from one that did, nothing is left to re-schedule,
 * and the native view keeps the previous rect while the pane believes it moved the
 * view — the stale rectangle this pane used to show. One extra placement per
 * settled layout is the only recovery that contract allows, and it costs a single
 * call per gesture.
 */
function scheduleSettle(): void {
  if (disposed) return;
  if (settleTimer !== null) window.clearTimeout(settleTimer);
  settleTimer = window.setTimeout(() => {
    settleTimer = null;
    // A transition burst is still moving the pane: let it finish first, so the
    // settle pass places the FINAL rect and not one frame of the animation.
    if (performance.now() < burstEnds) {
      scheduleSettle();
      return;
    }
    // Forgetting what was sent is what makes this pass unconditional: the rect may
    // be exactly the one above, and that is the case this pass exists for.
    sentRect = null;
    void syncBounds();
  }, SETTLE_MS);
}

/**
 * One layout signal from an observer. A size change is also the moment a view
 * that could not be built (the pane was collapsed to zero when it mounted) has
 * somewhere to live, so creation is retried here.
 */
function onLayoutSignal(): void {
  if (disposed) return;
  if (viewId.value === null && !creating.value) void createView();
  scheduleBounds();
}

/** Samples the rect while a transition runs, so the view follows it. */
function burstTick(): void {
  if (disposed) {
    burstTimer = null;
    return;
  }
  scheduleBounds();
  if (performance.now() < burstEnds) {
    burstTimer = window.setTimeout(burstTick, BOUNDS_FALLBACK_MS);
  } else {
    burstTimer = null;
  }
}

function onTransitionSignal(event: TransitionEvent): void {
  if (disposed) return;
  if (!GEOMETRY_PROPERTY.test(event.propertyName)) return;
  // Chained transitions (the sidebar, then the row that contains this pane)
  // extend the window instead of starting a second sampler.
  burstEnds = performance.now() + TRANSITION_BURST_MS;
  if (burstTimer === null) burstTick();
}

function watchLayout(): void {
  unwatchLayout();
  if (typeof ResizeObserver !== "undefined") {
    resizeObserver = new ResizeObserver(onLayoutSignal);
    if (bodyEl.value !== null) resizeObserver.observe(bodyEl.value);
    // The pane root too: dragging the drawer or re-splitting the dock changes our
    // position, and the body alone would only report the width it lost.
    if (rootEl.value !== null) resizeObserver.observe(rootEl.value);
  }
  window.addEventListener("resize", onLayoutSignal);
  // Capture: `scroll` does not bubble, and an ancestor scrolling moves the body
  // rect without resizing it.
  window.addEventListener("scroll", scheduleBounds, true);
  // Capture again: a transition inside any ancestor moves this pane, and the
  // observers above only report size changes.
  for (const type of ["transitionrun", "transitionend"] as const) {
    document.addEventListener(type, onTransitionSignal, true);
  }
}

function unwatchLayout(): void {
  if (resizeObserver !== null) {
    resizeObserver.disconnect();
    resizeObserver = null;
  }
  window.removeEventListener("resize", onLayoutSignal);
  window.removeEventListener("scroll", scheduleBounds, true);
  for (const type of ["transitionrun", "transitionend"] as const) {
    document.removeEventListener(type, onTransitionSignal, true);
  }
  if (burstTimer !== null) {
    window.clearTimeout(burstTimer);
    burstTimer = null;
  }
  // A queued frame or settle pass would measure a pane that is going away.
  cancelFrame();
  if (settleTimer !== null) {
    window.clearTimeout(settleTimer);
    settleTimer = null;
  }
}

/** Puts the address bar back in sync with the page, unless it is being edited. */
function syncAddress(url: string): void {
  currentUrl.value = url;
  if (addressFocused.value) return;
  draft.value = url === START_URL ? "" : url;
}

/** Every navigation of every view arrives here; only ours is kept. */
function handleNav(event: EmbedNavEvent): void {
  if (disposed || event.viewId !== viewId.value) return;
  syncAddress(event.url);
}

/**
 * Builds the child webview over the measured body rect. The registry entry is
 * created before the platform webview, so a create that fails leaves nothing
 * behind; a create that answers after teardown closes what it built.
 */
async function createView(): Promise<void> {
  if (
    viewId.value !== null ||
    creating.value ||
    disposed ||
    !drawn.value ||
    !shown.value
  ) {
    return;
  }
  creating.value = true;
  status.value = null;
  try {
    await nextTick();
    // One frame past `nextTick`: the drawer animates open and the dock settles
    // its split, and the rect read on the first tick is the pre-animation one.
    await nextFrame();
    if (disposed) return;
    const rect = bodyRect();
    // The pane is collapsed to nothing: there is no area to cover, so no view is
    // created. The next resize calls `createView` again.
    if (rect === null) return;
    const id = await embedCreate(
      rect.x,
      rect.y,
      rect.width,
      rect.height,
      START_URL,
    );
    // Torn down, or its tab was hidden while the platform was building it: close
    // it, because a hidden pane must not leave a webview painting over the app.
    // The drawer going away is the other case: creating is gated on `shown`, so a
    // view is never built behind a closed drawer; the resize observer that fires
    // when it reopens builds it then.
    if (disposed || !drawn.value || !shown.value) {
      void embedClose(id).catch(() => {});
      return;
    }
    viewId.value = id;
    sentRect = rect;
    syncAddress(await embedUrl(id).catch(() => START_URL));
    // The rect may have moved while the platform was building the webview.
    scheduleBounds();
  } catch (cause) {
    if (disposed) return;
    status.value = t("tools.browser.createError", {
      message: commandErrorMessage(cause),
    });
  } finally {
    if (!disposed) creating.value = false;
  }
}

/** Closes the view this pane owns, if any. Idempotent by construction. */
async function closeView(): Promise<void> {
  const id = viewId.value;
  viewId.value = null;
  // The next view is created over a fresh rect, so nothing is known about it.
  sentRect = null;
  if (id === null) return;
  try {
    await embedClose(id);
  } catch {
    // A view that is already gone is the outcome teardown wanted.
  }
}

/**
 * Navigates to what the address bar holds. The url is validated HERE first, so a
 * refused scheme shows its message and never crosses the bridge.
 */
async function go(): Promise<void> {
  if (draftEmpty.value) return;
  const target = normalizeAddress(draft.value);
  if (target === null) {
    status.value = t("tools.browser.urlInvalid");
    return;
  }
  const id = viewId.value;
  if (id === null || navigating.value) return;
  status.value = null;
  navigating.value = true;
  try {
    await embedNavigate(id, target);
    // The event confirms it; setting it now keeps the toolbar honest even if the
    // page is slow to report. `embed-nav` overwrites this with the real url.
    syncAddress(target);
  } catch (cause) {
    if (disposed) return;
    status.value = t("tools.browser.navigateError", {
      message: commandErrorMessage(cause),
    });
  } finally {
    if (!disposed) navigating.value = false;
  }
}

/**
 * Runs one page-level control inside the view. Back, Forward and Reload have no
 * command of their own, so they are `history.back()`, `history.forward()` and
 * `location.reload()` evaluated in the page.
 */
async function control(script: string): Promise<void> {
  const id = viewId.value;
  if (id === null) return;
  status.value = null;
  try {
    await embedEval(id, script);
  } catch (cause) {
    if (disposed) return;
    const message = commandErrorMessage(cause);
    // A reload (or a history step that leaves the document) tears down the JS
    // context before the evaluation can answer, so Rust reports its own timeout.
    // The page did what was asked; calling that a failure would be a lie.
    if (message.includes("did not answer")) return;
    status.value = t("tools.browser.controlError", { message });
  }
}

/** Subscribes to the navigation stream, then builds the view. */
async function start(): Promise<void> {
  watchLayout();
  try {
    const off = await subscribeEmbedNav(handleNav);
    if (disposed) {
      off();
      return;
    }
    unsubscribeNav = off;
  } catch (cause) {
    if (!disposed) {
      status.value = t("tools.browser.syncError", {
        message: commandErrorMessage(cause),
      });
    }
  }
  await createView();
}

/**
 * Rebuilds the pane for another folder. A workspace switch keeps this component
 * mounted (its tool context carries no path), so the view is closed here instead
 * of surviving into the next workspace's session.
 */
async function reset(): Promise<void> {
  if (disposed) return;
  await closeView();
  syncAddress(START_URL);
  draft.value = "";
  status.value = null;
  await createView();
}

// A workspace switch closes the view and builds a fresh one for the new folder.
watch(
  () => props.workspace?.path ?? null,
  (next, previous) => {
    if (next === previous) return;
    void reset();
  },
);

/**
 * Brings an already built view back on screen without rebuilding it.
 *
 * The platform still holds the rect from before the drawer collapsed, so the rect
 * is re-measured and applied FIRST and the view is shown afterwards: showing it
 * before would flash the page at the old position. Nothing is reloaded — the same
 * view, the same document.
 */
async function revealView(): Promise<void> {
  const id = viewId.value;
  if (id === null || disposed) return;
  // Unknown until the fresh rect is applied: the drawer's width survived, but the
  // chat column it shared the row with did not.
  sentRect = null;
  await nextTick();
  await nextFrame();
  if (disposed || viewId.value !== id || !shown.value) return;
  await syncBounds();
  // `syncBounds` is serialized: a call that lands while another one is in flight
  // is remembered and re-run, not applied now. A second pass one frame later means
  // the rect is on the platform before the view is shown.
  await nextFrame();
  if (disposed || viewId.value !== id) return;
  await syncBounds();
  if (disposed || viewId.value !== id) return;
  try {
    await embedSetVisible(id, true);
  } catch (cause) {
    if (disposed) return;
    if (isDeadView(cause)) {
      // Showing a view that is gone is the same dead view the bounds sync heals:
      // one code path, one rebuild — never a second creation path.
      healDeadView();
      return;
    }
    status.value = t("tools.browser.createError", {
      message: commandErrorMessage(cause),
    });
  }
}

/**
 * A hidden tab closes its view and a drawn one builds a fresh view. `post` so the
 * parent has already applied `v-show` and the body has a measurable rect.
 */
watch(
  drawn,
  (isDrawn) => {
    if (disposed) return;
    if (!isDrawn) {
      void closeView();
      return;
    }
    // Nothing queued from the hidden period would measure anything useful.
    sentRect = null;
    cancelFrame();
    void createView();
  },
  { flush: "post" },
);

/**
 * The drawer collapsed or reopened. This is NOT a close: the view is hidden with
 * the platform's own switch and shown again over a re-measured rect, so the page
 * the human was reading is still there (the reported bug: closing and reopening the
 * drawer restarted every tool). `viewId === null` means nothing is built yet and
 * creation is already gated on `shown`, so there is nothing to do.
 */
watch(
  shown,
  (visible) => {
    if (disposed) return;
    const id = viewId.value;
    if (id === null) return;
    if (visible) {
      void revealView();
      return;
    }
    void embedSetVisible(id, false).catch((cause: unknown) => {
      // The drawer is already hidden, so a status line here would be shown to
      // nobody. Showing the view again re-issues the call, and that path does
      // report its own failure — except for a view that is already gone, which is
      // cleared here so reopening the drawer builds a fresh one instead of trying
      // to show a dead id (and painting over the app while it does).
      if (!disposed && isDeadView(cause)) healDeadView();
    });
  },
  { flush: "post" },
);

onMounted(() => {
  void start();
});

onBeforeUnmount(() => {
  disposed = true;
  unwatchLayout();
  if (unsubscribeNav !== null) {
    unsubscribeNav();
    unsubscribeNav = null;
  }
  const id = viewId.value;
  viewId.value = null;
  if (id !== null) {
    // Best effort: `embed_close` is idempotent, and no child webview may outlive
    // the pane that placed it.
    void embedClose(id).catch(() => {});
  }
});
</script>

<template>
  <div ref="rootEl" class="flex min-h-0 flex-1 flex-col">
    <!-- Toolbar: real HTML, and the only part of this pane the webview never
         covers. Back/Forward/Reload act on the page, the field navigates. -->
    <div class="flex shrink-0 items-center gap-1 border-b px-1.5 py-1">
      <Button
        variant="ghost"
        size="icon-xs"
        :disabled="viewId === null"
        :aria-label="t('tools.browser.back')"
        :title="t('tools.browser.back')"
        @click="control('history.back()')"
      >
        <ArrowLeftIcon />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        :disabled="viewId === null"
        :aria-label="t('tools.browser.forward')"
        :title="t('tools.browser.forward')"
        @click="control('history.forward()')"
      >
        <ArrowRightIcon />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        :disabled="viewId === null"
        :aria-label="t('tools.browser.reload')"
        :title="t('tools.browser.reload')"
        @click="control('location.reload()')"
      >
        <RotateCwIcon />
      </Button>

      <form class="flex min-w-0 flex-1 items-center gap-1" @submit.prevent="go">
        <Input
          v-model="draft"
          class="h-7 min-w-0 flex-1 text-xs"
          type="text"
          inputmode="url"
          autocomplete="off"
          spellcheck="false"
          :placeholder="t('tools.browser.addressPlaceholder')"
          :aria-label="t('tools.browser.address')"
          :aria-invalid="addressInvalid"
          @focus="addressFocused = true"
          @blur="addressFocused = false"
        />
        <Button type="submit" size="xs" class="shrink-0" :disabled="!canGo">
          {{ t("tools.browser.go") }}
        </Button>
      </form>
    </div>

    <!-- Status: one line, in the pane's voice. The failure first, because it is
         the only thing the human has to act on. -->
    <p
      v-if="status"
      class="shrink-0 border-b px-2 py-1 text-xs text-destructive"
    >
      {{ status }}
    </p>
    <p
      v-else-if="creating"
      class="shrink-0 border-b px-2 py-1 text-xs text-muted-foreground"
    >
      {{ t("tools.browser.starting") }}
    </p>
    <p
      v-else-if="showingBlank"
      class="shrink-0 border-b px-2 py-1 text-xs text-muted-foreground"
    >
      {{ t("tools.browser.hint") }}
    </p>
    <p
      v-else
      class="shrink-0 truncate border-b px-2 py-1 text-xs text-muted-foreground"
      :title="currentUrl"
    >
      {{ currentUrl }}
    </p>

    <!-- BODY: the rect the child webview covers. Nothing here is visible under a
         live view, so the pane keeps one placeholder for the state where there is
         no webview at all. -->
    <div ref="bodyEl" class="relative min-h-0 flex-1 overflow-hidden">
      <p
        v-if="viewId === null && !creating"
        class="flex h-full items-center justify-center p-4 text-center text-xs text-muted-foreground"
      >
        {{ status ?? t("tools.browser.hint") }}
      </p>
    </div>

    <!-- The honest half: the app drives this view, an agent cannot reach it yet. -->
    <p class="shrink-0 border-t px-2 py-1.5 text-xs text-muted-foreground">
      {{ t("tools.browser.agentNote") }}
    </p>
  </div>
</template>
