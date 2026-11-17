// The drawer's tool registry (F1/F2 of `odd/tasks/tool-drawer-and-mcp.md`, and
// the dock of `src/components/tools/dock.ts`).
//
// One entry per tool is the whole integration point: the dock's "+ Add tool"
// menu is this list, and a pane mounts the `component` of the instance it holds.
// Adding a tool is one entry here, never an edit to the dock.
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
import type { Component } from "vue";
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
import TerminalTool from "./TerminalTool.vue";

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
  /** The pane icon; the components themselves are size-agnostic. */
  icon: Component;
  /** Mounted as-is by `DockPane.vue` with the props of its own context. */
  component: Component;
  /**
   * At most one pane may hold this tool. Adding it again FOCUSES the pane that
   * already has it instead of opening a second one, and a singleton pane cannot
   * be split: two views of the same working tree would only be two ways to look
   * at one answer.
   */
  singleton?: boolean;
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
    icon: GitBranchIcon,
    component: ChangesTool,
    singleton: true,
    defaultContext: (): ToolContext => ({}),
  },
  {
    id: "terminal",
    labelKey: "tools.terminal.label",
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
    icon: GlobeIcon,
    component: BrowserTool,
    // A pane owns a launched browser session; a second pane would launch a
    // second Chromium for the same workspace.
    singleton: true,
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
 * this one, and an empty answer is an empty menu (the dock then shows its empty
 * face) instead of a pane nobody asked for.
 */
export function visibleTools(enabled: readonly string[]): ToolDefinition[] {
  return TOOLS.filter((tool) => enabled.includes(tool.id));
}
