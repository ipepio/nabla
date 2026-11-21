/**
 * Zoom: the chord that changes it, the step it takes, and where the factor is
 * applied (maintainer request: «añadir en los atajos el cmd + y el cmd - para el
 * zoom y detectar si se está en windows o linux y en lugar de cmd que sea cntrl»).
 *
 * Three things are policy and live here, so the view owns no arithmetic:
 *
 * 1. **The chord.** `⌘` on macOS, `Ctrl` on Windows and Linux, with `=`/`+` to
 *    zoom in, `-`/`_` to zoom out and `0` to reset. `Alt` disqualifies a chord:
 *    `AltGr` types characters on Windows and Linux layouts, and swallowing those
 *    to zoom would be a bug the human blames on the keyboard.
 * 2. **The step.** A fixed list of levels, one step per key press, clamped at
 *    both ends — never a wrap-around and never a free-running factor, so
 *    pressing zoom-out ten times lands exactly on the minimum.
 * 3. **Which port receives the factor.** The native webview zoom is the real one:
 *    it re-lays out the page exactly as the engine would, and on this Tauri build
 *    it exists on all three desktop platforms (see `src/contracts/window.ts`). The
 *    document-level CSS `zoom` is the FALLBACK, used only when the native call
 *    answers "no": a plain browser during `vite dev`, or a platform that refuses
 *    it. It is deliberately sticky — once refused, the controller stops asking —
 *    and it is a degraded mode: `zoom` on the root element multiplies the used
 *    value of every length, so the app shell (`h-svh`) and the rect a child
 *    webview is placed with stop agreeing. That is why the native call is tried
 *    first and why the fallback is not the documented behaviour.
 */

/** Smallest level the list allows (50%). */
export const MIN_ZOOM = 0.5;
/** Largest level the list allows (200%). */
export const MAX_ZOOM = 2;
/** 100%: the level the app starts at and the one a reset returns to. */
export const DEFAULT_ZOOM = 1;

/**
 * The levels one key press walks through, in order. A short list of sane stops
 * instead of a continuous factor: every level is a size somebody actually wants,
 * and the ends are where the clamp sits.
 */
export const ZOOM_LEVELS: readonly number[] = [
  0.5, 0.67, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2,
];

/** What a zoom chord asks for. */
export type ZoomAction = "in" | "out" | "reset";

/** The families this build runs on; `other` gets the Windows/Linux modifier. */
export type ShortcutPlatform = "mac" | "windows" | "linux" | "other";

/**
 * The platform the chords follow, read from the webview the app runs in.
 *
 * `navigator.userAgent` is the primary source: WKWebView says `Macintosh`,
 * WebView2 says `Windows NT` and WebKitGTK says `Linux`. `navigator.platform` is
 * the second source, only consulted when the user agent names none of them (a
 * spoofed or trimmed agent), because it is deprecated in the DOM but still
 * present in all three engines.
 *
 * Anything unrecognised becomes `other`, which takes the `Ctrl` modifier: a new
 * platform should get a working chord, not none.
 */
export function detectPlatform(
  userAgent: string,
  platform = "",
): ShortcutPlatform {
  for (const source of [userAgent, platform]) {
    if (/macintosh|mac os x|macintel|darwin/i.test(source)) return "mac";
    if (/windows|win32|win64|wow64/i.test(source)) return "windows";
    if (/linux|x11/i.test(source)) return "linux";
  }
  return "other";
}

/** The subset of a keydown this module reads. */
export interface ZoomKeyEvent {
  key: string;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}

/** Which physical keys ask for which action. */
const ZOOM_KEYS: Record<string, ZoomAction> = {
  "=": "in",
  "+": "in",
  "-": "out",
  "_": "out",
  "0": "reset",
};

/**
 * The action a keydown asks for, or `null` when it is not a zoom chord.
 *
 * The modifier is the platform's own: `⌘` on macOS and `Ctrl` elsewhere. Holding
 * the other one is not a zoom — `Ctrl+=` is text editing on macOS, and `⌘=` is
 * nothing at all on Windows — so BOTH are exact, not "either of them".
 */
export function zoomAction(
  event: ZoomKeyEvent,
  platform: ShortcutPlatform,
): ZoomAction | null {
  if (event.altKey === true) return null;
  const required = platform === "mac" ? event.metaKey === true : event.ctrlKey === true;
  if (!required) return null;
  return ZOOM_KEYS[event.key] ?? null;
}

/**
 * One step from `current`. A level the list does not contain (a stale value, a
 * hand-written one) still steps to the next real level in the pressed direction
 * rather than being stranded, and the ends clamp.
 */
export function nextZoomLevel(current: number, action: ZoomAction): number {
  if (action === "reset") return DEFAULT_ZOOM;
  // A tolerance keeps 0.9 + 0.1 style drift from skipping or repeating a level.
  const epsilon = 1e-9;
  if (action === "in") {
    const above = ZOOM_LEVELS.find((level) => level > current + epsilon);
    return above ?? MAX_ZOOM;
  }
  for (let index = ZOOM_LEVELS.length - 1; index >= 0; index -= 1) {
    if (ZOOM_LEVELS[index] < current - epsilon) return ZOOM_LEVELS[index];
  }
  return MIN_ZOOM;
}

/**
 * How the three chords are written where a human reads them (the settings pane
 * and the README keymap). Same source as `zoomAction`, so the label cannot
 * describe a chord the app does not listen to.
 */
export function zoomShortcutLabels(
  platform: ShortcutPlatform,
): Record<ZoomAction, string> {
  const modifier = platform === "mac" ? "⌘" : "Ctrl";
  return {
    in: `${modifier} +`,
    out: `${modifier} −`,
    reset: `${modifier} 0`,
  };
}

/** The one property `applyDocumentZoom` writes; `zoom` is CSS, not a token. */
const ZOOM_PROPERTY = "zoom";

/** A root element, or anything a test can stand in for. */
export interface ZoomableRoot {
  style: {
    setProperty(name: string, value: string): void;
    removeProperty(name: string): void;
  };
}

/**
 * The fallback: scale the document itself. 100% removes the declaration instead
 * of writing `zoom: 1`, so a reset leaves the layout exactly as it shipped.
 */
export function applyDocumentZoom(root: ZoomableRoot, scale: number): void {
  if (scale === DEFAULT_ZOOM) {
    root.style.removeProperty(ZOOM_PROPERTY);
    return;
  }
  root.style.setProperty(ZOOM_PROPERTY, String(scale));
}

/** Where the factor goes. Both calls are best-effort and never throw. */
export interface ZoomPorts {
  /** Native webview zoom; resolves `false` when this build or platform refuses. */
  setNative(scale: number): Promise<boolean>;
  /** Document-level fallback, for when the native call refused. */
  setDocument(scale: number): void;
}

/** The current level, plus the one operation that changes it. */
export interface ZoomController {
  /** The level in force, as a factor (`1` is 100%). */
  level(): number;
  /** Applies one action and resolves to the level now in force. */
  apply(action: ZoomAction): Promise<number>;
}

function normaliseLevel(level: number): number {
  if (!Number.isFinite(level)) return DEFAULT_ZOOM;
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, level));
}

/**
 * The controller the shell owns: one level, two ports, and one decision — try
 * native first, and after the first refusal never ask again.
 *
 * A zoom that changes nothing (reset at 100%, zoom-in at the maximum) calls
 * NEITHER port: the level is the answer, and a bridge round trip that cannot
 * change the screen is waste, not correctness.
 */
export function createZoomController(
  ports: ZoomPorts,
  initial: number = DEFAULT_ZOOM,
): ZoomController {
  let current = normaliseLevel(initial);
  let native = true;
  return {
    level: () => current,
    async apply(action: ZoomAction): Promise<number> {
      const next = nextZoomLevel(current, action);
      if (next === current) return current;
      current = next;
      if (native) {
        let applied = false;
        try {
          applied = await ports.setNative(next);
        } catch {
          applied = false;
        }
        if (applied) return current;
        // Sticky: a platform without native zoom must not be asked per key press.
        native = false;
      }
      ports.setDocument(next);
      return current;
    },
  };
}
