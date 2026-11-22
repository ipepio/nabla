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
//   drawer        the whole right panel. It OPENS on its chooser: the first time
//                 the drawer is opened it holds one EMPTY tab, so choosing a tool
//                 is one click instead of "+ then pick" (S1 of
//                 `odd/tasks/drawer-responsiveness-oauth.md`). `openDrawer` is the
//                 only seeding path, and it seeds on the OPEN, never on a
//                 reconcile: a human who closes the last tab while the drawer is
//                 open gets the empty drawer back, not a fresh tab.
//   tab           a named area with its own layout. A new tab is EMPTY (`layout:
//                 null`) and shows the tool chooser; starting a tool turns that
//                 empty area into the tab's single pane. There is no limit on how
//                 many tabs a drawer holds.
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
// `startToolInTab`, `openTool`, `splitPane`, `closePane`, `setContextPath`,
// `focusPane`, `applyDock`. No component builds a `Pane` by hand, so there is
// exactly one place that can break the depth cap, the pane cap or the singleton
// rule.
//
// RECONCILE BEFORE WRITE — `applyDock` is the only thing that can empty the drawer,
// and it runs on the host's inputs (the active workspace and the enabled tools),
// which arrive ASYNCHRONOUSLY: the drawer mounts before either is known. A creation
// path that writes first and lets that reconcile run afterwards can have its tab
// cleared the moment the workspace settles. So the creation paths reconcile first
// (`openTool`) and `setTabTool` refuses to write into a drawer that is not current
// for the workspace it is being given: a tab may only be born in a drawer that
// already agrees about where it lives.
//
// THE TWO SINGLETON RULES live here and nowhere else: `singleton` is one pane per
// TAB, and `appWide` is one pane in the WHOLE drawer. Both are checked where the
// panes are created — `setTabTool` for a new pane, `splitPane` for a split —
// because this module is the only thing that can see every tab at once. A caller
// that needs to know which tab holds an app-wide tool asks
// `tabHoldingAppWideTool` instead of walking the list itself.
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

/** Every open tab, left to right. Empty until the drawer is first opened, or after
 * the human closes its last tab. */
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

/** The key `applyDock` stores for a workspace: the folder, or `null` for none. */
function workspaceKey(workspace: WorkspaceInfo | null): string | null {
  return workspace?.path ?? null;
}

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
 * Opens one empty tab when the drawer has none, so its first face is the tool
 * chooser and never a strip that only offers `+` (S1).
 *
 * Called when the drawer becomes visible (`App.vue`), and it RECONCILES FIRST for
 * the same reason `openTool` does: a drawer that is not current for its workspace
 * must be cleared before a tab is born in it, or the next reconcile erases the tab
 * that was just seeded.
 *
 * Where the seed deliberately does NOT live: inside `applyDock`. The reconcile runs
 * on inputs the drawer does not control (a settling workspace, a settings list),
 * and seeding there would reopen the tab the human just closed while the drawer was
 * on screen. Reopening the collapsed drawer runs this again, which is what S1 asks
 * for: opening the drawer always shows the chooser.
 *
 * Existing tabs are left exactly as they are — same ids, same active tab, same
 * panes — which is what makes collapsing and reopening free.
 */
export function openDrawer(
  workspace: WorkspaceInfo | null,
  enabled: readonly string[],
): void {
  applyDock(workspace, enabled);
  if (tabs.value.length === 0) addTab();
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
 * The tab holding a pane of one tool, or `undefined` when no tab does.
 *
 * This is the app-wide half of the singleton rule: a pane of a `singleton` tool is
 * unique inside its tab, while a pane of an `appWide` tool is unique in the whole
 * drawer. The agent path (`App.vue`, through `openTool`) uses this to ACTIVATE the
 * tab that already holds the browser instead of opening a second pane, and the
 * chooser uses it to send the human to that tab instead of refusing the click.
 *
 * It only ever sees tabs of the drawer this module is current for: `applyDock`
 * clears them on a workspace change and `setTabTool` refuses a write into a stale
 * one, so a pane counted here is a pane the panel draws.
 */
export function tabHoldingAppWideTool(toolId: ToolId): DockTab | undefined {
  return tabs.value.find((tab) =>
    tabPanes(tab).some((instance) => instance.toolId === toolId),
  );
}

/**
 * Starts a tool in an EMPTY tab: the empty area becomes the tab's single pane.
 * Refused for a tab that already holds a layout (a running tool is not replaced
 * silently; close it or close the tab), for an id the registry does not ship, for
 * a drawer that is not current for `workspace` (see below), and for an `appWide`
 * tool that already has a pane in ANY tab — the pane would be a second copy of the
 * same page or process, and the human must be sent to the tab that holds it (which
 * is what `tabHoldingAppWideTool` answers, and what `startToolInTab` does).
 */
export function setTabTool(
  tabId: string,
  toolId: ToolId,
  workspace: WorkspaceInfo | null,
): boolean {
  const tab = tabById(tabId);
  if (tab === undefined || tab.layout !== null) return false;
  const definition = toolById(toolId);
  if (definition === undefined) return false;
  // The tab must be born in the drawer that is CURRENT for its workspace. Writing
  // into a stale one is what made a pane unreachable: the pending reconcile (a
  // workspace that settled, a settings list that arrived) cleared the tab after
  // the fact, and until it ran the pane held an app-wide tool the panel never
  // drew. `openTool` and the chooser reconcile first, so this only answers a
  // caller that skipped it — and for that caller, refusing beats writing anyway.
  if (layoutWorkspace.value !== workspaceKey(workspace)) return false;
  // The app-wide rule, checked against every tab. The tab being filled is empty,
  // so it cannot be the one holding it: a second pane is the only way this fires.
  if (
    definition.appWide === true &&
    tabHoldingAppWideTool(toolId) !== undefined
  ) {
    return false;
  }
  const instance = makeInstance(toolId, workspace);
  replaceTab(tab.id, { ...tab, layout: { kind: "leaf", instance } });
  activeTabId.value = tab.id;
  focusedId.value = instance.id;
  return true;
}

/**
 * Starts one tool in an EMPTY tab on the HUMAN's behalf (a click in the chooser),
 * and does what the app-wide rule asks instead of only saying no: when the tool
 * already lives in another tab of the drawer, THAT tab is activated and the empty
 * tab the chooser belongs to goes away, because the request "show me the browser"
 * is already satisfied. A second pane is never created, so 45e9a1d's invariant
 * holds no matter how many tabs the human opens.
 *
 * Answers `true` when the tool is on screen afterwards.
 */
export function startToolInTab(
  tabId: string,
  toolId: ToolId,
  workspace: WorkspaceInfo | null,
): boolean {
  const tab = tabById(tabId);
  if (tab === undefined) return false;
  if (toolById(toolId)?.appWide === true) {
    const holder = tabHoldingAppWideTool(toolId);
    if (holder !== undefined && holder.id !== tab.id) {
      activateTab(holder.id);
      // The chooser only exists in an empty tab, and an empty tab left behind is
      // litter that offers nothing the tab on screen does not already show.
      if (tab.layout === null) closeTab(tab.id);
      return true;
    }
  }
  return setTabTool(tabId, toolId, workspace);
}

/**
 * The one entry point for "open this tool for `workspace`" when the caller has no
 * tab in mind — the agent's `embed-need-view` path in `App.vue`.
 *
 * It RECONCILES FIRST and writes second, and that ordering is the whole point: the
 * reconcile that a workspace change (or a settings list that just arrived) triggers
 * would otherwise run after the write and clear the tab that was just created —
 * leaving the drawer open, empty, and the request silently lost. Reconciling first
 * also means the app-wide lookup below can only see panes the panel draws, and that
 * the tab is created in a drawer that is already current for its workspace, so any
 * later reconcile with the same inputs is a no-op.
 *
 * An app-wide tool that already lives in the drawer is REVEALED, never duplicated;
 * otherwise the drawer's EMPTY active tab becomes the tool, and only when there is
 * none (or the active tab already holds a layout) does a new tab open for it. That
 * second rule is what keeps `openDrawer`'s chooser tab from being left behind as an
 * orphan when the agent opens a pane in a drawer that was just opened.
 *
 * Answers the tab showing the tool, or `null` when the registry does not ship it
 * (nothing is left behind in that case).
 */
export function openTool(
  toolId: ToolId,
  workspace: WorkspaceInfo | null,
  enabled: readonly string[],
): string | null {
  // The same inputs the host reconciles on, applied NOW instead of after this
  // write. A pending workspace switch therefore clears the drawer BEFORE the tab
  // exists, which is the only ordering in which a new tab cannot be wiped.
  applyDock(workspace, enabled);
  const definition = toolById(toolId);
  if (definition === undefined) return null;
  if (definition.appWide === true) {
    const holder = tabHoldingAppWideTool(toolId);
    if (holder !== undefined) {
      activateTab(holder.id);
      return holder.id;
    }
  }
  const current = activeTab();
  const tabId =
    current !== undefined && current.layout === null ? current.id : addTab();
  if (!startToolInTab(tabId, toolId, workspace)) {
    // Nothing was started: no empty tab is left behind as the trace of the failure.
    closeTab(tabId);
    return null;
  }
  return tabId;
}

/**
 * Opens or focuses a Document pane pointed at `path`:
 * - If a Document pane already holds that exact `path`, focuses its tab.
 * - If a Document pane is empty (`path === ""`), loads `path` into it.
 * - Otherwise opens a new Document tab so existing open documents stay untouched.
 */
export function openDocumentInDrawer(
  path: string,
  workspace: WorkspaceInfo | null,
  enabled: readonly string[],
): string | null {
  const trimmed = path.trim();
  applyDock(workspace, enabled);
  if (!enabled.includes("document")) return null;

  let emptyTarget: { tabId: string; paneId: string } | null = null;
  for (const tab of tabs.value) {
    if (tab.layout === null) continue;
    for (const slot of leafSlots(tab.layout)) {
      if (slot.instance.toolId !== "document") continue;
      if (slot.instance.context.path === trimmed) {
        activeTabId.value = tab.id;
        focusedId.value = slot.instance.id;
        return tab.id;
      }
      if (emptyTarget === null && slot.instance.context.path === "") {
        emptyTarget = { tabId: tab.id, paneId: slot.instance.id };
      }
    }
  }

  if (emptyTarget !== null) {
    const tab = tabById(emptyTarget.tabId);
    if (tab && tab.layout !== null) {
      const targetPaneId = emptyTarget.paneId;
      const layout = mapLeaves(tab.layout, (leaf) =>
        leaf.instance.id === targetPaneId && leaf.instance.toolId === "document"
          ? {
              kind: "leaf",
              instance: {
                ...leaf.instance,
                context: { path: trimmed },
              },
            }
          : leaf,
      );
      replaceTab(tab.id, { ...tab, layout });
      activeTabId.value = tab.id;
      focusedId.value = targetPaneId;
      return tab.id;
    }
  }

  const tabId = openTool("document", workspace, enabled);
  if (tabId === null) return null;
  const tab = tabById(tabId);
  if (tab && tab.layout !== null) {
    const layout = mapLeaves(tab.layout, (leaf) =>
      leaf.instance.toolId === "document"
        ? {
            kind: "leaf",
            instance: {
              ...leaf.instance,
              context: { path: trimmed },
            },
          }
        : leaf,
    );
    replaceTab(tab.id, { ...tab, layout });
  }
  return tabId;
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
 *
 * A creation path that must not be erased by this call runs it FIRST — see
 * `openTool` — so that "reconcile" and "write" can never race.
 */
export function applyDock(
  workspace: WorkspaceInfo | null,
  enabled: readonly string[],
): void {
  const key = workspaceKey(workspace);
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
