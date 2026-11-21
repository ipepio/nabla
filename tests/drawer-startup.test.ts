// Drawer startup: the drawer opens on its chooser tab, and the boot settings read
// is handed to the chat instead of being repeated
// (`odd/tasks/drawer-responsiveness-oauth.md`, S1 and S2).
//
// The drawer's model lives in `src/components/tools/dock.ts`, behind the registry,
// which imports `.vue` components and the app's `@/` alias — two things plain Node
// cannot resolve on its own. Both are presentation-only for this file: the tab
// model never renders a component, so a stub component and an alias mapping are
// enough to exercise the real dock. The same hook records which `.vue` modules are
// evaluated at import time, which is how the deferred Terminal load is observed
// from the outside.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { registerHooks } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  rememberBootSettings,
  takeBootSettings,
} from "../src/lib/boot-settings.ts";
import type { AppSettings, WorkspaceInfo } from "../src/contracts/types.ts";

const projectRoot = pathToFileURL(
  path.resolve(import.meta.dirname, "..") + path.sep,
).href;

/** Every `.vue` module Node evaluated, in load order. */
const loadedVue: string[] = [];

registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier.startsWith("@/")) {
      return nextResolve(projectRoot + specifier.slice(2), context);
    }
    if (specifier.endsWith(".vue")) {
      return {
        url: new URL(specifier, context.parentURL).href,
        shortCircuit: true,
      };
    }
    // The app resolves extensionless relative imports (a bundler rule); Node does
    // not, so a `.ts` sibling is the same module the bundler would pick.
    if (!path.extname(specifier) && specifier.startsWith(".")) {
      const candidate = new URL(specifier + ".ts", context.parentURL);
      if (existsSync(fileURLToPath(candidate))) {
        return nextResolve(candidate.href, context);
      }
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url.endsWith(".vue")) {
      // A name is all the registry needs from a component here: the chooser shows
      // an icon and a label, and the dock never mounts a pane in these tests.
      loadedVue.push(path.basename(fileURLToPath(url)));
      return {
        format: "module",
        source: "export default { name: 'ToolStub', render: () => null };",
        shortCircuit: true,
      };
    }
    return nextLoad(url, context);
  },
});

const dock = await import("../src/components/tools/dock.ts");

/** A workspace is only ever read through its folder by the dock. */
function workspace(folder: string): WorkspaceInfo {
  return { path: folder } as WorkspaceInfo;
}

const DEFAULT_TOOLS = ["files", "changes"];
const ALL_TOOLS = ["files", "changes", "terminal", "browser"];

function toolIds(tab: (typeof dock.tabs.value)[number]): string[] {
  return dock.tabPanes(tab).map((instance) => instance.toolId);
}

test("the drawer's first open shows the chooser, not a bare strip with a plus", () => {
  const folder = workspace("/drawer/startup/first-open");
  dock.applyDock(folder, DEFAULT_TOOLS);
  assert.equal(dock.tabs.value.length, 0);

  dock.openDrawer(folder, DEFAULT_TOOLS);

  assert.equal(dock.tabs.value.length, 1);
  const tab = dock.tabs.value[0];
  assert.equal(tab.layout, null);
  assert.equal(dock.activeTabId.value, tab.id);
});

test("opening again does not pile up chooser tabs", () => {
  const folder = workspace("/drawer/startup/one-tab");
  dock.applyDock(folder, DEFAULT_TOOLS);

  dock.openDrawer(folder, DEFAULT_TOOLS);
  dock.openDrawer(folder, DEFAULT_TOOLS);

  assert.equal(dock.tabs.value.length, 1);
});

test("a drawer emptied while open stays empty; only an open seeds a tab", () => {
  const folder = workspace("/drawer/startup/close-last");
  dock.applyDock(folder, DEFAULT_TOOLS);
  dock.openDrawer(folder, DEFAULT_TOOLS);

  dock.closeTab(dock.tabs.value[0].id);
  assert.equal(dock.tabs.value.length, 0);

  // A reconcile can land while the drawer is open (a flipped tool switch, a
  // workspace that settles); it must not reopen what the human closed.
  dock.applyDock(folder, ALL_TOOLS);
  assert.equal(dock.tabs.value.length, 0);
});

test("reopening keeps the tabs and the active one", () => {
  const folder = workspace("/drawer/startup/reopen");
  dock.applyDock(folder, DEFAULT_TOOLS);
  dock.openDrawer(folder, DEFAULT_TOOLS);
  dock.startToolInTab(dock.tabs.value[0].id, "files", folder);
  const filesTab = dock.tabs.value[0].id;

  dock.addTab();
  dock.startToolInTab(dock.tabs.value[1].id, "changes", folder);
  const changesTab = dock.tabs.value[1].id;

  dock.activateTab(filesTab);
  const panesBefore = dock.tabPanes(dock.activeTab()).map((pane) => pane.id);

  // Collapsing the drawer is not an event the model sees: reopening reconciles
  // with the same inputs and must find the drawer exactly as it left it.
  dock.openDrawer(folder, DEFAULT_TOOLS);

  assert.deepEqual(
    dock.tabs.value.map((tab) => tab.id),
    [filesTab, changesTab],
  );
  assert.equal(dock.activeTabId.value, filesTab);
  assert.deepEqual(
    dock.tabPanes(dock.activeTab()).map((pane) => pane.id),
    panesBefore,
  );
});

test("an agent open fills the chooser tab instead of leaving it behind", () => {
  const folder = workspace("/drawer/startup/agent-fills");
  dock.applyDock(folder, DEFAULT_TOOLS);
  dock.openDrawer(folder, DEFAULT_TOOLS);
  const chooserTab = dock.tabs.value[0].id;

  const opened = dock.openTool("files", folder, DEFAULT_TOOLS);

  assert.equal(opened, chooserTab);
  assert.equal(dock.tabs.value.length, 1);
  assert.deepEqual(toolIds(dock.tabs.value[0]), ["files"]);
});

test("the browser is still revealed, never duplicated, across tabs", () => {
  const folder = workspace("/drawer/startup/browser-reveal");
  dock.applyDock(folder, ALL_TOOLS);

  const first = dock.openTool("browser", folder, ALL_TOOLS);
  dock.addTab();
  const second = dock.openTool("browser", folder, ALL_TOOLS);

  assert.equal(first, second);
  assert.equal(dock.activeTabId.value, first);
  const browsers = dock.tabs.value.flatMap((tab) =>
    toolIds(tab).filter((id) => id === "browser"),
  );
  assert.equal(browsers.length, 1);
});

test("the Terminal bundle is not loaded until a terminal pane exists", () => {
  // The hook above sees every `.vue` the registry pulls in, so an eager import
  // would have been recorded by now; Files proves the observation is not empty.
  assert.equal(loadedVue.includes("FilesTool.vue"), true);
  assert.equal(loadedVue.includes("TerminalTool.vue"), false);
});

test("the boot read is handed to the chat exactly once", () => {
  const read = { language: "es" } as AppSettings;

  assert.equal(takeBootSettings(), null);
  rememberBootSettings(read);
  assert.equal(takeBootSettings(), read);
  // A later read is a fresh read: the cached payload must not outlive startup.
  assert.equal(takeBootSettings(), null);
});
