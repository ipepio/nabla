// The drawer's tool registry (F1/F2 of `odd/tasks/tool-drawer-and-mcp.md`).
//
// One entry per tool is the whole integration point: `ToolTabs` renders its tab
// strip from this list and mounts the `component` of the active one, so adding a
// tool is one entry here and never an edit to the tab host.
//
// Two rules keep this list honest:
//
// 1. A tool is listed only when it has a component. A tab that does nothing is
//    worse than no tab, and `browser` (F3) therefore has no entry yet: its
//    component does not exist, so declaring it here would be a dead tab. The
//    `enabledTools` switch already exists in Settings (`src/lib/preferences.ts`),
//    so the flag is ready before the tab is.
// 2. Whether a listed tool is SHOWN is `enabledTools` (S5), which the host reads
//    through `visibleTools` below — never a build flag. A tool that is off simply
//    does not reach the strip.
//
// Nothing else lives in this file: the registry is data, and every tool keeps its
// own state and its own requests.
import type { Component } from "vue";
import { FilesIcon, GitBranchIcon, TerminalIcon } from "@lucide/vue";
import type { MessageKey } from "@/lib/i18n";
import ChangesTool from "./ChangesTool.vue";
import FilesTool from "./FilesTool.vue";
import TerminalTool from "./TerminalTool.vue";

/**
 * The tools this build ships. `browser` is deliberately absent: F3 adds it here
 * together with its component (see rule 1 above).
 */
export type ToolId = "files" | "changes" | "terminal";

/** One tool the drawer can show. */
export interface ToolDefinition {
  /** The id `enabledTools` uses for this tool. */
  id: ToolId;
  /** The tab label, resolved from the catalogue so both locales apply. */
  labelKey: MessageKey;
  /** The tab icon; the components themselves are size-agnostic. */
  icon: Component;
  /** Mounted as-is with `workspace` as its only prop. */
  component: Component;
}

/**
 * The tools in tab order. `files` first because it is the one the user reaches
 * for while a session runs; `terminal` last because it is opt-in and runs code.
 */
export const TOOLS: readonly ToolDefinition[] = [
  {
    id: "files",
    labelKey: "tools.files.label",
    icon: FilesIcon,
    component: FilesTool,
  },
  {
    id: "changes",
    labelKey: "tools.changes.label",
    icon: GitBranchIcon,
    component: ChangesTool,
  },
  {
    id: "terminal",
    labelKey: "tools.terminal.label",
    icon: TerminalIcon,
    component: TerminalTool,
  },
];

/**
 * S5: the tools the human enabled, in registry order. An id in `enabled` that no
 * entry declares is ignored, so a settings file from a newer build never breaks
 * this one, and an empty answer is an empty strip (the host then shows its empty
 * face) instead of a tab nobody asked for.
 */
export function visibleTools(enabled: readonly string[]): ToolDefinition[] {
  return TOOLS.filter((tool) => enabled.includes(tool.id));
}

/** The tool opened when the drawer has nothing remembered, or nothing to use. */
export const DEFAULT_TOOL_ID: ToolId = TOOLS[0].id;
