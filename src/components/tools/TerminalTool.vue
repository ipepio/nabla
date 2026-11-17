<script setup lang="ts">
// Terminal tab (F2 of `odd/tasks/tool-drawer-and-mcp.md`): a real shell in the
// active workspace, rendered by xterm.js.
//
// LIFECYCLE — one shell per mounting, and mounting IS activation. `ToolTabs`
// mounts only the active tool and keys it by (tool, workspace), so this component
// is created when the human opens the Terminal tab and destroyed when they leave
// it or switch workspace. That gives the whole rule for free:
//
//   mount      → open the session (`createShell` with the workspace path)
//   unmount    → `closeShell` (the PTY child dies with the tab)
//   resubscribe→ the `shell-output` listener is created once and dropped on
//                unmount, so nothing keeps firing after teardown
//
// The exit of the child is NOT unmount: `exit` keeps the scrollback on screen and
// offers Restart, which opens a fresh session in the same directory.
//
// DATA FLOW — bytes go to the emulator exactly as they came off the PTY
// (`data` chunks, no line splitting: xterm needs the raw carriage returns and
// escape sequences), and keystrokes go back through `writeShell` unmangled.
// Clipboard is xterm's own: Cmd/Ctrl+C copies the selection, Cmd/Ctrl+V pastes
// into the session.
//
// SIZING — the emulator is fitted to its container (fit addon) and the resulting
// geometry is pushed to the PTY through `resizeShell`, debounced so a drag of the
// panel handle does not produce one ioctl per pixel. `fit()` changes `term.cols`,
// which fires `onResize`; that handler is the only place that talks to the PTY, so
// there is exactly one direction of sizing and no feedback loop.
//
// COLOURS — read from the app's CSS tokens (`--background`, `--foreground`,
// `--muted`) and converted to sRGB through a 1×1 canvas: xterm's own colour
// parser only understands hex/rgb(), and the palette is authored in `oklch()`.
// Painting the colour and reading the pixel back is what makes both themes
// readable instead of silently falling back to xterm's defaults.
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import type { WorkspaceInfo, ShellOutputEvent } from "@/contracts/types";
import {
  closeShell,
  commandErrorMessage,
  createShell,
  resizeShell,
  subscribeShellOutput,
  writeShell,
} from "@/contracts/commands";
import { useAppearance } from "@/lib/appearance";
import { t } from "@/lib/i18n";
import { Button } from "@/components/ui/button";

const props = defineProps<{
  workspace: WorkspaceInfo | null;
}>();

/** Rows kept above the viewport. Generous, but bounded: a chatty build log ends. */
const SCROLLBACK = 5_000;
/** How long the panel may keep resizing before the PTY is told the new size. */
const RESIZE_DEBOUNCE_MS = 120;

/** The element xterm is opened into. */
const container = ref<HTMLDivElement | null>(null);
/** Phase flags the template renders. */
const starting = ref(false);
const ended = ref(false);
const exitCode = ref<number | null>(null);
const error = ref<string | null>(null);

let term: Terminal | null = null;
let fitAddon: FitAddon | null = null;
let observer: ResizeObserver | null = null;
let resizeTimer: number | null = null;
let unsubscribe: (() => void) | null = null;
/** The session this tab owns; `null` while starting, and after teardown. */
let sessionId: string | null = null;
/** `true` between `createShell` and its answer, so early output is not lost. */
let awaitingSession = false;
/** Output that arrived before the session id did (see `awaitingSession`). */
let earlyOutput: ShellOutputEvent[] = [];
/** Set on unmount: late answers must not touch a disposed emulator. */
let disposed = false;

const { isDark } = useAppearance();

/**
 * A CSS colour the browser can paint, as `rgb()`/`rgba()`. The canvas is the
 * conversion: `getPropertyValue` hands back the authored `oklch(...)` string and
 * xterm's parser would reject it.
 */
function toSrgb(value: string, fallback: string): string {
  if (value.trim() === "") return fallback;
  const canvas = document.createElement("canvas");
  canvas.width = 1;
  canvas.height = 1;
  const context = canvas.getContext("2d");
  if (context === null) return fallback;
  // Two assignments: an unparsable value leaves the previous one in place, so the
  // fallback is what the first `fillStyle` sets.
  context.fillStyle = fallback;
  context.fillStyle = value;
  context.fillRect(0, 0, 1, 1);
  const [red, green, blue, alpha] = context.getImageData(0, 0, 1, 1).data;
  if (alpha === 0) return fallback;
  return `rgba(${red}, ${green}, ${blue}, ${(alpha / 255).toFixed(3)})`;
}

/** One semantic token, resolved for the palette that is painted right now. */
function token(name: string, fallback: string): string {
  if (typeof window === "undefined") return fallback;
  const raw = getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim();
  return toSrgb(raw, fallback);
}

/**
 * The emulator theme, built from the app tokens so the terminal belongs to the
 * same surface as the rest of the drawer in both palettes. The fallbacks are the
 * dark values, which is the palette the drawer ships with.
 */
function buildTheme(): ITheme {
  const background = token("--background", "#252525");
  const foreground = token("--foreground", "#fafafa");
  return {
    background,
    foreground,
    cursor: foreground,
    cursorAccent: background,
    selectionBackground: token("--muted", "#454545"),
    selectionForeground: foreground,
  };
}

/** Write one chunk, ignoring anything that arrives after teardown. */
function writeChunk(data: string): void {
  if (disposed || term === null) return;
  term.write(data);
}

/**
 * One event of the stream every session shares. Events of other sessions are
 * dropped here: the listener is single, the ownership is per tab.
 */
function applyEvent(event: ShellOutputEvent): void {
  if (sessionId === null || event.sessionId !== sessionId) return;
  if (event.stream === "data") {
    if (typeof event.data === "string") writeChunk(event.data);
    return;
  }
  // The child is gone. The scrollback stays on screen; Restart opens a new one.
  ended.value = true;
  exitCode.value = event.exitCode ?? null;
  sessionId = null;
}

/**
 * The single subscription. Output produced between the spawn and the answer of
 * `createShell` is buffered instead of dropped: a prompt printed immediately would
 * otherwise be invisible. `exit` is buffered too, because a shell that dies on
 * spawn would otherwise leave a tab that looks alive.
 */
function handleEvent(event: ShellOutputEvent): void {
  if (sessionId === null) {
    if (awaitingSession) earlyOutput.push(event);
    return;
  }
  applyEvent(event);
}

/** Debounced fit: a drag of the panel handle resizes the emulator once it stops. */
function scheduleFit(): void {
  if (resizeTimer !== null) window.clearTimeout(resizeTimer);
  resizeTimer = window.setTimeout(() => {
    resizeTimer = null;
    fit();
  }, RESIZE_DEBOUNCE_MS);
}

function fit(): void {
  if (fitAddon === null) return;
  try {
    fitAddon.fit();
  } catch {
    // A container with no measurable size (the drawer mid-collapse) is not an
    // error; the next resize observer tick fits again.
  }
}

/** Creates the emulator and opens it into its container. */
function mountTerminal(): void {
  const element = container.value;
  if (element === null) return;
  term = new Terminal({
    cursorBlink: true,
    convertEol: false,
    scrollback: SCROLLBACK,
    fontFamily:
      'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace',
    fontSize: 12,
    lineHeight: 1.2,
    allowProposedApi: false,
    theme: buildTheme(),
  });
  fitAddon = new FitAddon();
  term.loadAddon(fitAddon);
  term.open(element);
  // Keystrokes go to the PTY verbatim, and only while it is alive.
  term.onData((data) => {
    if (sessionId === null) return;
    void writeShell(sessionId, data).catch(() => {
      // A rejected write means the session just ended; the `exit` event that
      // follows is what the UI reacts to.
    });
  });
  // The single sizing direction: emulator → PTY.
  term.onResize(({ cols, rows }) => {
    if (sessionId === null) return;
    void resizeShell(sessionId, cols, rows).catch(() => {
      // Same as a rejected write: the session is already gone.
    });
  });
  fit();
  term.focus();
}

/** Opens the PTY for this workspace and adopts its id. */
async function startSession(): Promise<void> {
  const workspace = props.workspace;
  if (workspace === null) return;
  error.value = null;
  ended.value = false;
  exitCode.value = null;
  starting.value = true;
  awaitingSession = true;
  earlyOutput = [];
  try {
    const session = await createShell({ workspace: workspace.path });
    if (disposed) {
      // The tab closed while the shell was spawning: do not leak the child.
      void closeShell(session.sessionId).catch(() => {});
      return;
    }
    sessionId = session.sessionId;
    awaitingSession = false;
    const buffered = earlyOutput;
    earlyOutput = [];
    for (const event of buffered) applyEvent(event);
    // The initial geometry, in case `fit()` landed on exactly the emulator's
    // default size and `onResize` therefore never fired.
    if (term !== null) {
      void resizeShell(sessionId, term.cols, term.rows).catch(() => {});
    }
    term?.focus();
  } catch (cause) {
    if (disposed) return;
    error.value = commandErrorMessage(cause);
    awaitingSession = false;
    earlyOutput = [];
  } finally {
    if (!disposed) starting.value = false;
  }
}

/** Discards the current emulator content and opens a fresh session. */
async function restart(): Promise<void> {
  const abandoned = sessionId;
  sessionId = null;
  ended.value = false;
  exitCode.value = null;
  if (abandoned !== null) {
    void closeShell(abandoned).catch(() => {});
  }
  term?.reset();
  await startSession();
  fit();
}

onMounted(async () => {
  if (props.workspace === null) return;
  mountTerminal();
  // Subscribe before spawning: the first prompt is output too.
  try {
    unsubscribe = await subscribeShellOutput(handleEvent);
  } catch {
    // Without the stream the tab cannot work; `startSession` still reports why.
    unsubscribe = null;
  }
  if (disposed) {
    unsubscribe?.();
    unsubscribe = null;
    return;
  }
  await startSession();
  const element = container.value;
  if (element !== null && typeof ResizeObserver !== "undefined") {
    observer = new ResizeObserver(() => scheduleFit());
    observer.observe(element);
  }
});

// Both palettes stay readable: the theme is rebuilt whenever the palette flips.
watch(isDark, () => {
  if (term === null) return;
  term.options.theme = buildTheme();
}, { flush: "post" });

onBeforeUnmount(() => {
  disposed = true;
  if (resizeTimer !== null) {
    window.clearTimeout(resizeTimer);
    resizeTimer = null;
  }
  observer?.disconnect();
  observer = null;
  unsubscribe?.();
  unsubscribe = null;
  awaitingSession = false;
  earlyOutput = [];
  const open = sessionId;
  sessionId = null;
  if (open !== null) {
    // Best effort: `closeShell` is idempotent and the child must not outlive the
    // tab that asked for it.
    void closeShell(open).catch(() => {});
  }
  term?.dispose();
  term = null;
  fitAddon = null;
});
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div
      v-if="workspace === null"
      class="flex flex-1 items-center justify-center p-4"
    >
      <p class="text-center text-xs text-muted-foreground">
        {{ t("tools.terminal.noWorkspace") }}
      </p>
    </div>

    <div v-else class="flex min-h-0 flex-1 flex-col">
      <!--
        The emulator surface. xterm owns everything inside this element; the
        overlays below are siblings so they never end up in the scrollback.
      -->
      <div
        ref="container"
        role="group"
        :aria-label="t('tools.terminal.aria')"
        class="min-h-0 flex-1 overflow-hidden px-1.5 py-1"
      />

      <p
        v-if="starting && !ended && error === null"
        class="shrink-0 border-t px-2 py-1 text-xs text-muted-foreground"
      >
        {{ t("tools.terminal.starting") }}
      </p>

      <p
        v-else-if="error"
        class="shrink-0 border-t px-2 py-1 text-xs text-destructive"
      >
        {{ t("tools.terminal.error", { message: error }) }}
      </p>

      <!-- The child left: the scrollback stays, and one button opens a new one. -->
      <div
        v-else-if="ended"
        class="flex shrink-0 items-center gap-2 border-t px-2 py-1.5"
      >
        <p class="min-w-0 flex-1 truncate text-xs text-muted-foreground">
          {{
            exitCode === null
              ? t("tools.terminal.endedUnknown")
              : t("tools.terminal.ended", { code: exitCode })
          }}
        </p>
        <Button variant="outline" size="xs" @click="restart">
          {{ t("tools.terminal.restart") }}
        </Button>
      </div>
    </div>
  </div>
</template>
