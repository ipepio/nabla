// Zoom shortcuts: which chord zooms, by how much, and where the factor is
// applied (S1 of `odd/tasks/zoom-shortcuts.md`).
//
// Everything here is policy, not plumbing, so it is exercised through
// `src/lib/zoom.ts`: the chord mapping is a pure function of the keyboard event
// and the platform, the stepping is a pure function of the current level, and the
// controller is observed through the two ports the view supplies — a native
// webview zoom and the document-level fallback. The real Tauri call and the real
// DOM write live behind those ports and are not reached from a test.
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  applyDocumentZoom,
  createZoomController,
  DEFAULT_ZOOM,
  detectPlatform,
  MAX_ZOOM,
  MIN_ZOOM,
  nextZoomLevel,
  zoomAction,
  zoomShortcutLabels,
} from "../src/lib/zoom.ts";

const MAC_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
const WINDOWS_UA =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36 Edg/126.0";
const LINUX_UA =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

/** A keydown as the platform delivers it: the modifier, the key, nothing else. */
function chord(
  key: string,
  modifiers: { meta?: boolean; ctrl?: boolean; alt?: boolean; shift?: boolean },
): {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
} {
  return {
    key,
    metaKey: modifiers.meta === true,
    ctrlKey: modifiers.ctrl === true,
    altKey: modifiers.alt === true,
    shiftKey: modifiers.shift === true,
  };
}

// --- Platform detection -----------------------------------------------------

test("the platform is read from the webview's user agent", () => {
  assert.equal(detectPlatform(MAC_UA), "mac");
  assert.equal(detectPlatform(WINDOWS_UA), "windows");
  assert.equal(detectPlatform(LINUX_UA), "linux");
  // An agent this does not know gets the Windows/Linux modifier, never no chord.
  assert.equal(detectPlatform("Mozilla/5.0 (Unknown)"), "other");
  assert.equal(detectPlatform(""), "other");
});

test("a user agent without a platform falls back to navigator.platform", () => {
  assert.equal(detectPlatform("", "MacIntel"), "mac");
  assert.equal(detectPlatform("", "Win32"), "windows");
  assert.equal(detectPlatform("", "Linux x86_64"), "linux");
});

// --- Chord → action ---------------------------------------------------------

test("on macOS the command key is the zoom modifier", () => {
  assert.equal(zoomAction(chord("=", { meta: true }), "mac"), "in");
  assert.equal(zoomAction(chord("+", { meta: true, shift: true }), "mac"), "in");
  assert.equal(zoomAction(chord("-", { meta: true }), "mac"), "out");
  assert.equal(zoomAction(chord("0", { meta: true }), "mac"), "reset");
});

test("on Windows and Linux control is the zoom modifier", () => {
  assert.equal(zoomAction(chord("=", { ctrl: true }), "windows"), "in");
  assert.equal(zoomAction(chord("+", { ctrl: true }), "windows"), "in");
  assert.equal(zoomAction(chord("-", { ctrl: true }), "linux"), "out");
  assert.equal(zoomAction(chord("0", { ctrl: true }), "linux"), "reset");
  assert.equal(zoomAction(chord("0", { ctrl: true }), "windows"), "reset");
  // Shift turns `-` into `_` on a US layout, and that is the same key.
  assert.equal(zoomAction(chord("_", { ctrl: true, shift: true }), "windows"), "out");
});

test("the other platform's modifier, and no modifier at all, zoom nothing", () => {
  // Ctrl is a text-editing modifier on macOS; Command is not the Windows/Linux one.
  assert.equal(zoomAction(chord("=", { ctrl: true }), "mac"), null);
  assert.equal(zoomAction(chord("=", { meta: true }), "windows"), null);
  assert.equal(zoomAction(chord("=", { meta: true }), "linux"), null);
  assert.equal(zoomAction(chord("=", {}), "mac"), null);
  assert.equal(zoomAction(chord("0", {}), "windows"), null);
});

test("alt chords are left alone: AltGr types characters", () => {
  assert.equal(zoomAction(chord("=", { meta: true, alt: true }), "mac"), null);
  assert.equal(zoomAction(chord("-", { ctrl: true, alt: true }), "linux"), null);
});

test("a key that is not part of the chord zooms nothing", () => {
  assert.equal(zoomAction(chord("a", { meta: true }), "mac"), null);
  assert.equal(zoomAction(chord("9", { ctrl: true }), "windows"), null);
  assert.equal(zoomAction(chord("Equal", { ctrl: true }), "windows"), null);
});

// --- Stepping ---------------------------------------------------------------

test("zoom in and out walk the levels one step at a time", () => {
  assert.equal(DEFAULT_ZOOM, 1);
  assert.equal(nextZoomLevel(1, "in"), 1.1);
  assert.equal(nextZoomLevel(1.1, "in"), 1.25);
  assert.equal(nextZoomLevel(1, "out"), 0.9);
  assert.equal(nextZoomLevel(0.9, "out"), 0.8);
});

test("the ends of the range are clamps, not wrap-arounds", () => {
  assert.equal(nextZoomLevel(MAX_ZOOM, "in"), MAX_ZOOM);
  assert.equal(nextZoomLevel(MIN_ZOOM, "out"), MIN_ZOOM);
});

test("reset returns to 100% from anywhere", () => {
  assert.equal(nextZoomLevel(MIN_ZOOM, "reset"), DEFAULT_ZOOM);
  assert.equal(nextZoomLevel(MAX_ZOOM, "reset"), DEFAULT_ZOOM);
  assert.equal(nextZoomLevel(1, "reset"), DEFAULT_ZOOM);
});

test("a level the list does not contain steps to the next real one", () => {
  // Nothing in the app writes 1.05, but a stale level must not strand the human.
  assert.equal(nextZoomLevel(1.05, "in"), 1.1);
  assert.equal(nextZoomLevel(1.05, "out"), 1);
});

// --- Labels the UI and the docs show ---------------------------------------

test("the labels name the modifier of the platform the app runs on", () => {
  const mac = zoomShortcutLabels("mac");
  assert.match(mac.in, /⌘/);
  assert.match(mac.out, /−/);
  assert.match(mac.reset, /0/);

  for (const platform of ["windows", "linux", "other"] as const) {
    const labels = zoomShortcutLabels(platform);
    assert.match(labels.in, /^Ctrl /, `got ${labels.in}`);
    assert.match(labels.out, /^Ctrl /, `got ${labels.out}`);
    assert.match(labels.reset, /^Ctrl /, `got ${labels.reset}`);
    assert.notEqual(labels.in, mac.in);
  }
});

// --- The document-level fallback ------------------------------------------

test("the fallback zooms the document and reset removes the declaration", () => {
  const root = fakeRoot();
  applyDocumentZoom(root, 1.25);
  assert.equal(root.properties.get("zoom"), "1.25");

  applyDocumentZoom(root, DEFAULT_ZOOM);
  assert.equal(
    root.properties.get("zoom"),
    undefined,
    "100% is the absence of the declaration, not `zoom: 1`",
  );
  assert.equal(root.removals, 1);
});

// --- The controller: which port receives the factor -----------------------

test("a native zoom that answers keeps the fallback out of it", async () => {
  const ports = fakePorts({ native: true });
  const zoom = createZoomController(ports);

  assert.equal(await zoom.apply("in"), 1.1);
  assert.deepEqual(ports.nativeCalls, [1.1]);
  assert.deepEqual(ports.documentCalls, []);
  assert.equal(zoom.level(), 1.1);
});

test("a refused native zoom falls back to the document and stops asking", async () => {
  const ports = fakePorts({ native: false });
  const zoom = createZoomController(ports);

  assert.equal(await zoom.apply("in"), 1.1);
  assert.deepEqual(ports.nativeCalls, [1.1]);
  assert.deepEqual(
    ports.documentCalls,
    [1.1],
    "the refused level is applied through the fallback immediately",
  );

  assert.equal(await zoom.apply("in"), 1.25);
  assert.deepEqual(ports.nativeCalls, [1.1], "the bridge is not asked a second time");
  assert.deepEqual(ports.documentCalls, [1.1, 1.25]);
});

test("a zoom that changes nothing calls neither port", async () => {
  const ports = fakePorts({ native: true });
  const zoom = createZoomController(ports);

  assert.equal(await zoom.apply("reset"), DEFAULT_ZOOM, "already at 100%");
  assert.deepEqual(ports.nativeCalls, []);
  assert.deepEqual(ports.documentCalls, []);

  for (let step = 0; step < 20; step += 1) await zoom.apply("in");
  assert.equal(zoom.level(), MAX_ZOOM);
  const callsAtTop = ports.nativeCalls.length;
  assert.equal(await zoom.apply("in"), MAX_ZOOM);
  assert.equal(ports.nativeCalls.length, callsAtTop, "the top is not re-sent");
});

test("reset returns to 100% through the port already in use", async () => {
  const ports = fakePorts({ native: false });
  const zoom = createZoomController(ports);

  await zoom.apply("in");
  await zoom.apply("in");
  assert.equal(await zoom.apply("reset"), DEFAULT_ZOOM);
  assert.deepEqual(ports.documentCalls, [1.1, 1.25, DEFAULT_ZOOM]);
  assert.deepEqual(ports.nativeCalls, [1.1]);
});

test("the controller starts at 100% unless it is handed another level", async () => {
  const ports = fakePorts({ native: true });
  assert.equal(createZoomController(ports).level(), DEFAULT_ZOOM);
  const zoom = createZoomController(ports, 1.5);
  assert.equal(zoom.level(), 1.5);
  assert.equal(await zoom.apply("in"), 1.75);

  // A level outside anything the list holds is repaired, never trusted.
  assert.equal(createZoomController(ports, Number.NaN).level(), DEFAULT_ZOOM);
  assert.equal(createZoomController(ports, 12).level(), MAX_ZOOM);
});

// --- Fakes -----------------------------------------------------------------

interface FakePorts {
  nativeCalls: number[];
  documentCalls: number[];
  setNative: (scale: number) => Promise<boolean>;
  setDocument: (scale: number) => void;
}

function fakePorts(options: { native: boolean }): FakePorts {
  const ports: FakePorts = {
    nativeCalls: [],
    documentCalls: [],
    setNative: async (scale: number) => {
      ports.nativeCalls.push(scale);
      return options.native;
    },
    setDocument: (scale: number) => {
      ports.documentCalls.push(scale);
    },
  };
  return ports;
}

function fakeRoot(): {
  properties: Map<string, string>;
  removals: number;
  style: {
    setProperty(name: string, value: string): void;
    removeProperty(name: string): void;
  };
} {
  const root = {
    properties: new Map<string, string>(),
    removals: 0,
    style: {
      setProperty(name: string, value: string): void {
        root.properties.set(name, value);
      },
      removeProperty(name: string): void {
        root.removals += 1;
        root.properties.delete(name);
      },
    },
  };
  return root;
}
