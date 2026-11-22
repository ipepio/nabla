// The drawer's tool registry (F1/F2 of `odd/tasks/tool-drawer-and-mcp.md`, and
// the dock of `src/components/tools/dock.ts`).
//
// One entry per tool is the whole integration point: the tool chooser of an empty
// tab is this list, and a pane mounts the `component` of the instance it holds.
// Adding a tool is one entry here, never an edit to the drawer.
//
// Two rules keep this list honest:
//
// 1. A tool is listed only when it has a component. A menu entry that does
//    nothing is worse than no entry, so an id reaches this list together with its
//    component and never before it. The `enabledTools` switch already exists in
//    Settings (`src/lib/preferences.ts`), so the flag may be ready before it is.
// 2. Whether a listed tool is offered is `enabledTools` (S5), which the dock
//    reads through `visibleTools` below — never a build flag. A tool that is off
//    never reaches the menu, and a pane holding it is closed by the dock.
//
// The CONTEXT is the per-pane half of the model. An instance is not "the Files
// tool": it is a Files tool pointed at one folder, and two instances may point at
// two folders. `defaultContext` is what a fresh instance starts as — normally the
// active workspace, which is the folder the human already chose — and the dock
// lets that value be changed per pane afterwards (that is what "open another path
// in another pane" means).
//
// Nothing else lives in this file: the registry is data, and every tool keeps its
// own state and its own requests.
import { defineAsyncComponent, type Component } from "vue";
import {
  FilesIcon,
  GitBranchIcon,
  GlobeIcon,
  TerminalIcon,
} from "@lucide/vue";
import type { WorkspaceInfo } from "@/contracts/types";
import type { MessageKey } from "@/lib/i18n";
import BrowserTool from "./BrowserTool.vue";
import ChangesTool from "./ChangesTool.vue";
import FilesTool from "./FilesTool.vue";

/**
 * The terminal is the one tool whose code is not in the drawer's startup bundle.
 *
 * `TerminalTool.vue` pulls in xterm and its fit addon, and Terminal is OFF by
 * default (S5): most sessions never open a shell, so the editor, the parser and
 * the fonts it carries were downloaded and evaluated on every start for a pane
 * nobody asked for. A dynamic import moves that work to the moment the first
 * terminal pane mounts (S2 of `odd/tasks/drawer-responsiveness-oauth.md`), and the
 * dock needed no change: `DockPane.vue` already mounts `component` through `<component
 * :is>`, which renders an async component as soon as its chunk arrives.
 *
 * The other three stay eager on purpose: Files and Changes are the panes a session
 * opens first, and Browser coordinates its child webview with the drawer's first
 * paint — neither may wait for a second round-trip.
 */
const TerminalTool = defineAsyncComponent(() => import("./TerminalTool.vue"));

/**
 * The context of one tool instance, per tool. Files browses a folder, Terminal
 * runs a shell in one, and the two read-only views need no context of their own
 * (they read the active workspace).
 */
export interface ToolContexts {
  files: { rootPath: string };
  terminal: { cwd: string };
  changes: Record<string, never>;
  browser: Record<string, never>;
}

/** The tools this build ships; each id matches an `enabledTools` switch (S5). */
export type ToolId = keyof ToolContexts;

/** Any context, for code that handles the union without knowing the tool. */
export type ToolContext = ToolContexts[ToolId];

/**
 * One open pane: a tool plus the context it is showing. Discriminated by
 * `toolId`, so reading `context` without checking the id cannot compile.
 */
export type ToolInstance = {
  [K in ToolId]: { id: string; toolId: K; context: ToolContexts[K] };
}[ToolId];

/** One tool the dock can open. */
export interface ToolDefinition {
  /** The id `enabledTools` uses for this tool. */
  id: ToolId;
  /** The pane label, resolved from the catalogue so both locales apply. */
  labelKey: MessageKey;
  /** Short description shown in the empty-tab tool chooser. */
  descriptionKey: MessageKey;
  /** The pane icon; the components themselves are size-agnostic. */
  icon: Component;
  /** Mounted as-is by `DockPane.vue` with the props of its own context. */
  component: Component;
  /**
   * At most one pane of a TAB may hold this tool, and a singleton pane cannot be
   * split: two views of the same working tree would only be two ways to look at
   * one answer. Tabs stay independent — a second tab is a second area, which is
   * the whole point of the tab strip.
   */
  singleton?: boolean;
  /**
   * At most one pane in the WHOLE drawer, across every tab. A different fact from
   * `singleton`, and strictly stronger in practice: a tool that owns one page or
   * one process cannot be duplicated by opening another tab, because a second pane
   * would be a second one nobody can tell apart from the first.
   *
   * The rule is enforced in `dock.ts` (`setTabTool`), which is the only place that
   * can see every tab at once; the chooser only stops offering the tool once a
   * pane of it already exists.
   */
  appWide?: boolean;
  /** The context a fresh pane starts with, normally derived from the workspace. */
  defaultContext(workspace: WorkspaceInfo | null): ToolContext;
}

/**
 * The tools in menu order. `files` first because it is the one the user reaches
 * for while a session runs; the two opt-in tools — the terminal and the browser —
 * last, because both are off by default and this order is the menu order once
 * Settings turns them on.
 */
export const TOOLS: readonly ToolDefinition[] = [
  {
    id: "files",
    labelKey: "tools.files.label",
    descriptionKey: "settings.general.tools.files.description",
    icon: FilesIcon,
    component: FilesTool,
    // One pane, one folder. Splitting a Files pane copies its root and the human
    // then points the new pane somewhere else.
    defaultContext: (workspace: WorkspaceInfo | null): ToolContext => ({
      rootPath: workspace?.path ?? "",
    }),
  },
  {
    id: "changes",
    labelKey: "tools.changes.label",
    descriptionKey: "settings.general.tools.changes.description",
    icon: GitBranchIcon,
    component: ChangesTool,
    singleton: true,
    defaultContext: (): ToolContext => ({}),
  },
  {
    id: "terminal",
    labelKey: "tools.terminal.label",
    descriptionKey: "settings.general.tools.terminal.description",
    icon: TerminalIcon,
    component: TerminalTool,
    // A split terminal is a second shell, which is the whole point of splitting
    // it: two prompts in two folders.
    defaultContext: (workspace: WorkspaceInfo | null): ToolContext => ({
      cwd: workspace?.path ?? "",
    }),
  },
  {
    id: "browser",
    labelKey: "tools.browser.label",
    descriptionKey: "settings.general.tools.browser.description",
    icon: GlobeIcon,
    component: BrowserTool,
    // A pane owns one page and one session; a second pane — in this tab OR in
    // another one — would be a second page the human cannot tell apart from the
    // first, and the agent could not say which one it drove.
    singleton: true,
    appWide: true,
    defaultContext: (): ToolContext => ({}),
  },
];

/** One entry by id, or `undefined` for an id this build does not ship. */
export function toolById(id: string): ToolDefinition | undefined {
  return TOOLS.find((tool) => tool.id === id);
}

/**
 * S5: the tools the human enabled, in registry order. An id in `enabled` that no
 * entry declares is ignored, so a settings file from a newer build never breaks
 * this one, and an empty answer is an empty chooser (the tab then says where the
 * switches are) instead of a pane nobody asked for.
 */
export function visibleTools(enabled: readonly string[]): ToolDefinition[] {
  return TOOLS.filter((tool) => enabled.includes(tool.id));
}
