// The drawer's tool registry (F1 of `odd/tasks/tool-drawer-and-mcp.md`).
//
// One entry per tool is the whole integration point: `ToolTabs` renders its tab
// strip from this list and mounts the `component` of the active one, so adding a
// tool is one entry here and never an edit to the tab host.
//
// Terminal and Browser (F2, F3 of the same document) will be added here as two
// more entries once their Rust side exists. They are deliberately not listed as
// disabled tabs: a tab that does nothing is worse than no tab, and their
// `enabledTools` flags are off by default.
//
// Nothing else lives in this file: the registry is data, and every tool keeps its
// own state and its own requests.
import type { Component } from "vue";
import { FilesIcon, GitBranchIcon } from "@lucide/vue";
import type { MessageKey } from "@/lib/i18n";
import ChangesTool from "./ChangesTool.vue";
import FilesTool from "./FilesTool.vue";

/** The tools F1 ships. F2/F3 widen this union with `terminal` and `browser`. */
export type ToolId = "files" | "changes";

/** One tool the drawer can show. */
export interface ToolDefinition {
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
 * for while a session runs.
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
];

/** The tool opened on the first render of the drawer. */
export const DEFAULT_TOOL_ID: ToolId = TOOLS[0].id;
