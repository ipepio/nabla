// The drawer's layout model: TABS, and inside each tab a split tree of panes.
//
// The request behind this file is literal: "No quiero tener un máximo de tools, mi
// idea es que el drawer esté vacío y pueda seleccionar las tools que quiera, en el
// drawer que pueda tener pestañas, estas pestañas es un área vacía y puedo arrancar
// la tool que esté disponible. Y ya después, poder dividir una pestaña hasta un
// máximo de 4."
//
// THE HIERARCHY — drawer → tabs → panes:
//
//   drawer        the whole right panel. Starts EMPTY: no tab is open, and no tool
//                 is chosen for the human. The `+` at the end of the strip adds
//                 one, and there is no limit on how many.
//   tab           a named area with its own layout. A new tab is EMPTY (`layout:
//                 null`) and shows the tool chooser; starting a tool turns that
//                 empty area into the tab's single pane.
//   pane          one tool instance. A tab's layout is a binary split tree: a leaf
//                 is one instance, a split is two children laid out in a row (side
//                 by side) or a column (stacked). Splitting a leaf nests one more
//                 level, so the tree IS the layout and the panes are its leaves.
//
// WHY A TREE AND NOT A GRID — the human thinks in "split right / split down", and
// that is exactly a binary split of the pane they are looking at. A grid would
// have to invent a cell model for a narrow drawer nobody needs.
//
// THE CAP IS PER TAB — depth 2, so at most 4 panes in ONE tab (`MAX_PANES`), and
// never a global limit: five tabs of four panes are fine. The cap is enforced in
// two places: a leaf at depth 2 cannot be split (`canSplit` in `DockPane.vue` hides
// the buttons) and `splitPane` refuses the fifth pane here, which the model owns.
//
// STATE — module scope on purpose. The drawer unmounts when it is collapsed (the
// parent keeps only `toolsOpen`), so a component-local `ref` would throw the tabs
// away every time the panel is hidden. Tabs survive collapsing, tab switches, pane
// splits and Settings flips; they are cleared only when the ACTIVE WORKSPACE
// changes, because panes are pointed at one workspace's folders and a shell belongs
// to the session it was opened in. Nothing is persisted (no `settings.json` key).
//
// SINGLE WRITER — every mutation of the model goes through a function here: the
// host renders `tabs`/`activeTabId` and calls `addTab`, `closeTab`, `setTabTool`,
// `splitPane`, `closePane`, `setContextPath`, `focusPane`, `applyDock`. No component
// builds a `Pane` by hand, so there is exactly one place that can break the depth
// cap, the pane cap or the singleton rule.
import { ref } from "vue";
import type { WorkspaceInfo } from "@/contracts/types";
import type { MessageKey } from "@/lib/i18n";
import {
  toolById,
  type ToolContext,
  type ToolId,
  type ToolInstance,
} from "./registry";

/** Splits above a leaf: 0 is the root pane, 2 is the deepest pane of a tab. */
export const MAX_DEPTH = 2;

/** At most 4 panes in one tab, which is the whole tree depth 2 can hold. */
export const MAX_PANES = 1 << MAX_DEPTH;

/** How a split lays its two children out. */
export type SplitDirection = "row" | "col";

/** One pane of docked tool. */
export type LeafPane = { kind: "leaf"; instance: ToolInstance };

/** Two panes side by side (`row`) or stacked (`col`). */
export type SplitPane = {
  kind: "split";
  direction: SplitDirection;
  children: [Pane, Pane];
};

export type Pane = LeafPane | SplitPane;

/** One tab of the drawer: a name and its own split tree (or an empty area). */
export interface DockTab {
  id: string;
  /**
   * The human-facing name. Every tab is named after the tool of its first pane
   * (`tabTitleKey`), in the current locale, so this stays `null` today: it is the
   * override slot a rename would use, and deriving the label instead of storing it
   * is what keeps a language switch from freezing a tab's name.
   */
  title: string | null;
  /** `null` is the empty tab: it shows the chooser and holds no tool yet. */
  layout: Pane | null;
}

/** Every open tab, left to right. Empty on a fresh start by design. */
export const tabs = ref<DockTab[]>([]);

/** The tab whose body is drawn. `null` only when there is no tab at all. */
export const activeTabId = ref<string | null>(null);

/**
 * The pane a split starts from, and the one drawn as focused. Kept as an id and
 * not as a reference: the tree is replaced on every change, so a stored node
 * would go stale.
 */
export const focusedId = ref<string | null>(null);

/** The workspace the current tabs were built for (see `applyDock`). */
const layoutWorkspace = ref<string | null>(null);

/** Instance ids are local and monotonic: stable keys for Vue, nothing durable. */
let instanceSequence = 0;
let tabSequence = 0;

function nextInstanceId(): string {
  instanceSequence += 1;
  return `pane-${instanceSequence}`;
}

function nextTabId(): string {
  tabSequence += 1;
  return `tab-${tabSequence}`;
}

/**
 * A fresh instance of one tool, with the context its registry entry defaults to.
 *
 * The registry types `defaultContext` per tool and the id picks the union member
 * at runtime, so the compiler cannot pair them by itself; the cast below is that
 * one pairing, and it is the only one in the dock.
 */
function makeInstance(
  toolId: ToolId,
  workspace: WorkspaceInfo | null,
): ToolInstance {
  const definition = toolById(toolId);
  const context = (definition?.defaultContext(workspace) ?? {}) as ToolContext;
  return { id: nextInstanceId(), toolId, context } as ToolInstance;
}

/** One tab by id, or `undefined` when it is not open. */
export function tabById(id: string | null): DockTab | undefined {
  if (id === null) return undefined;
  return tabs.value.find((tab) => tab.id === id);
}

/** The tab being drawn, or `undefined` when the drawer has no tab at all. */
export function activeTab(): DockTab | undefined {
  return tabById(activeTabId.value);
}

interface LeafSlot {
  instance: ToolInstance;
  /** Splits above this leaf; `0` is the root pane of its tab. */
  depth: number;
}

function leafSlots(
  tree: Pane | null,
  depth = 0,
  out: LeafSlot[] = [],
): LeafSlot[] {
  if (tree === null) return out;
  if (tree.kind === "leaf") {
    out.push({ instance: tree.instance, depth });
    return out;
  }
  leafSlots(tree.children[0], depth + 1, out);
  leafSlots(tree.children[1], depth + 1, out);
  return out;
}

/** Every instance of one tab, left to right, in render order. */
export function tabPanes(tab: DockTab | undefined): ToolInstance[] {
  if (tab === undefined) return [];
  return leafSlots(tab.layout).map((slot) => slot.instance);
}

/** The i18n key of the label this tab's title shows. */
export function tabTitleKey(tab: DockTab): MessageKey {
  const first = leafSlots(tab.layout)[0];
  if (first === undefined) return "tools.tabs.new";
  return toolById(first.instance.toolId)?.labelKey ?? "tools.tabs.new";
}

/**
 * Replaces one tab through `next`, keeping the array (and therefore every other
 * tab's component tree and its tool state) untouched.
 */
function replaceTab(id: string, next: DockTab): void {
  tabs.value = tabs.value.map((tab) => (tab.id === id ? next : tab));
}

/**
 * Rewrite every leaf through `fn`, dropping the leaves it answers `null` for.
 *
 * A split whose child disappeared is replaced by the surviving one, so removing a
 * pane never leaves an empty half behind: the tree stays "every split has two
 * children", which is what the renderer and the cap assume.
 */
function mapLeaves(tree: Pane, fn: (leaf: LeafPane) => Pane | null): Pane | null {
  if (tree.kind === "leaf") return fn(tree);
  const left = mapLeaves(tree.children[0], fn);
  const right = mapLeaves(tree.children[1], fn);
  if (left === null) return right;
  if (right === null) return left;
  return { kind: "split", direction: tree.direction, children: [left, right] };
}

/** Replaces the target leaf with a split holding it and `instance`. */
function splitAt(
  tree: Pane,
  targetId: string,
  direction: SplitDirection,
  instance: ToolInstance,
): Pane | null {
  const twin: Pane = { kind: "leaf", instance };
  return mapLeaves(tree, (leaf) =>
    leaf.instance.id === targetId
      ? {
          kind: "split",
          direction,
          children: [{ kind: "leaf", instance: leaf.instance }, twin],
        }
      : leaf,
  );
}

/**
 * Opens a new, EMPTY tab at the end of the strip and switches to it, so the `+` is
 * one click from the chooser. There is no limit on tabs (the request says so
 * explicitly: "No quiero tener un máximo de tools"); the only cap in the model is
 * the panes of one tab.
 */
export function addTab(): string {
  const tab: DockTab = { id: nextTabId(), title: null, layout: null };
  tabs.value = [...tabs.value, tab];
  activeTabId.value = tab.id;
  // An empty tab has no pane to focus.
  focusedId.value = null;
  return tab.id;
}

/** Switches the drawn tab, and puts the focus on a pane of it. */
export function activateTab(id: string): void {
  const tab = tabById(id);
  if (tab === undefined) return;
  activeTabId.value = tab.id;
  const panes = tabPanes(tab);
  if (!panes.some((instance) => instance.id === focusedId.value)) {
    focusedId.value = panes[0]?.id ?? null;
  }
}

/**
 * Closes one tab and its panes. The neighbour that takes its place becomes active;
 * closing the last tab leaves the drawer with no tab at all, which is the "no tabs"
 * empty state with the `+` next to it, never an error.
 */
export function closeTab(id: string): void {
  const index = tabs.value.findIndex((tab) => tab.id === id);
  if (index === -1) return;
  const wasActive = activeTabId.value === id;
  tabs.value = tabs.value.filter((tab) => tab.id !== id);
  const neighbour = tabs.value[index] ?? tabs.value[index - 1];
  if (wasActive) activeTabId.value = neighbour?.id ?? null;
  // The focus may have lived in the tab that just went away.
  const panes = tabPanes(activeTab());
  if (!panes.some((instance) => instance.id === focusedId.value)) {
    focusedId.value = panes[0]?.id ?? null;
  }
}

/**
 * Starts a tool in an EMPTY tab: the empty area becomes the tab's single pane.
 * Refused for a tab that already holds a layout (a running tool is not replaced
 * silently; close it or close the tab) and for an id the registry does not ship.
 */
export function setTabTool(
  tabId: string,
  toolId: ToolId,
  workspace: WorkspaceInfo | null,
): boolean {
  const tab = tabById(tabId);
  if (tab === undefined || tab.layout !== null) return false;
  if (toolById(toolId) === undefined) return false;
  const instance = makeInstance(toolId, workspace);
  replaceTab(tab.id, { ...tab, layout: { kind: "leaf", instance } });
  activeTabId.value = tab.id;
  focusedId.value = instance.id;
  return true;
}

/**
 * Splits one pane in two inside its tab. The new pane shows the same tool with the
 * same context, which is the useful starting point: a second Files pane on the same
 * folder, one "Browse" away from a different one; a second shell in the same
 * directory, one `cd` away from another.
 *
 * Refused — and the buttons are hidden, so it cannot be clicked — when the tool is
 * a singleton, when the pane is already at the depth cap, or when THAT TAB already
 * holds `MAX_PANES` panes. Other tabs are never consulted.
 */
export function splitPane(
  tabId: string,
  instanceId: string,
  direction: SplitDirection,
): void {
  const tab = tabById(tabId);
  if (tab === undefined || tab.layout === null) return;
  const slots = leafSlots(tab.layout);
  if (slots.length >= MAX_PANES) return;
  const slot = slots.find((candidate) => candidate.instance.id === instanceId);
  if (slot === undefined || slot.depth >= MAX_DEPTH) return;
  if (toolById(slot.instance.toolId)?.singleton === true) return;
  const twin = {
    ...slot.instance,
    id: nextInstanceId(),
    context: { ...slot.instance.context },
  } as ToolInstance;
  const layout = splitAt(tab.layout, instanceId, direction, twin);
  if (layout === null) return;
  replaceTab(tab.id, { ...tab, layout });
  focusedId.value = twin.id;
}

/**
 * Closes one pane and collapses its tab's tree around the gap. The LAST pane of a
 * tab is never closed here: a tool needs somewhere to live, and the way to get rid
 * of the tab is closing the tab (which is why the pane button hides itself).
 */
export function closePane(tabId: string, instanceId: string): void {
  const tab = tabById(tabId);
  if (tab === undefined || tab.layout === null) return;
  if (tab.layout.kind === "leaf") return;
  const layout = mapLeaves(tab.layout, (leaf) =>
    leaf.instance.id === instanceId ? null : leaf,
  );
  replaceTab(tab.id, { ...tab, layout });
  if (focusedId.value === instanceId) {
    focusedId.value = leafSlots(layout)[0]?.instance.id ?? null;
  }
}

/** The folder one pane is pointed at, or `null` for a context-free tool. */
export function contextPath(instance: ToolInstance): string | null {
  switch (instance.toolId) {
    case "files":
      return instance.context.rootPath;
    case "terminal":
      return instance.context.cwd;
    default:
      return null;
  }
}

/**
 * Points one pane at another folder. This is how a second Files pane (or a second
 * shell) gets its OWN root instead of the workspace's: the context is per instance,
 * and the pane remounts on it because the mount key includes it.
 */
export function setContextPath(
  tabId: string,
  instanceId: string,
  path: string,
): void {
  const tab = tabById(tabId);
  if (tab === undefined || tab.layout === null) return;
  const trimmed = path.trim();
  if (trimmed === "") return;
  const layout = mapLeaves(tab.layout, (leaf) => {
    if (leaf.instance.id !== instanceId) return leaf;
    const instance = leaf.instance;
    if (instance.toolId === "files") {
      return {
        kind: "leaf",
        instance: {
          ...instance,
          context: { ...instance.context, rootPath: trimmed },
        },
      };
    }
    if (instance.toolId === "terminal") {
      return {
        kind: "leaf",
        instance: { ...instance, context: { ...instance.context, cwd: trimmed } },
      };
    }
    // Changes and Browser read the active workspace; a path is not theirs.
    return leaf;
  });
  replaceTab(tab.id, { ...tab, layout });
}

/** Marks one pane as the one a split starts from, and as the drawn-focused one. */
export function focusPane(instanceId: string): void {
  focusedId.value = instanceId;
}

/**
 * Reconciles the tabs with the things they depend on. Called by the host on mount
 * and whenever either input changes (the drawer being collapsed is not a change:
 * the model lives at module scope and is still here when it reopens).
 *
 *  1. A new active workspace clears the drawer: every pane was pointed at a folder
 *     of the old one, and a shell belongs to the session it was opened in. The
 *     drawer then shows its own empty state — a workspace switch is not the moment
 *     to open a tool nobody asked for.
 *  2. A tool the human disabled closes its panes, and each tab's tree collapses
 *     around them (S5: an off tool is never mounted, which is also what keeps the
 *     terminal from opening a shell nobody asked for). A tab left without panes
 *     becomes an empty tab again, showing the chooser.
 *
 * It deliberately does NOT open anything: the drawer starts empty and stays as
 * empty as the human left it (the request: "mi idea es que el drawer esté vacío").
 */
export function applyDock(
  workspace: WorkspaceInfo | null,
  enabled: readonly string[],
): void {
  const key = workspace?.path ?? null;
  if (layoutWorkspace.value !== key) {
    layoutWorkspace.value = key;
    tabs.value = [];
    activeTabId.value = null;
    focusedId.value = null;
    return;
  }
  const available = new Set(enabled);
  tabs.value = tabs.value.map((tab) => {
    if (tab.layout === null) return tab;
    const layout = mapLeaves(tab.layout, (leaf) =>
      available.has(leaf.instance.toolId) ? leaf : null,
    );
    return layout === tab.layout ? tab : { ...tab, layout };
  });
  if (!tabs.value.some((tab) => tab.id === activeTabId.value)) {
    activeTabId.value = tabs.value[0]?.id ?? null;
  }
  // The focus must be a pane of the drawn tab: a closed pane took it away.
  const panes = tabPanes(activeTab());
  if (!panes.some((instance) => instance.id === focusedId.value)) {
    focusedId.value = panes[0]?.id ?? null;
  }
}
