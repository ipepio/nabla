// The dock's layout model (the request behind this file: "un selector de tools y
// que tú puedas abrir las que quieras en el drawer… ¿y si quiero abrir otro path
// porque quiero abrir otro archivo? ¿y si quiero dividir la pestaña del drawer
// porque quiero 2 o n files? Lo mismo con la consola").
//
// WHAT A DOCK IS — not a tab strip. The drawer holds any number of tool
// INSTANCES arranged as a small split tree: a leaf is one instance, a split is two
// children laid out in a row (side by side) or a column (stacked). Splitting a
// leaf again nests one more level, so the tree is the layout and the panes are its
// leaves.
//
// WHY A TREE AND NOT A GRID — the human thinks in "split right / split down", and
// that is exactly a binary split of the pane they are looking at. A grid would
// have to invent a cell model for a 280px-wide drawer nobody needs.
//
// THE CAP — depth 2, so at most 4 panes (`MAX_PANES`). The drawer is narrow: past
// four panes every pane is a sliver. The cap is enforced in two places: a leaf at
// depth 2 cannot be split (`canSplit` in `DockPane.vue` hides the buttons) and
// `addTool` refuses to open a fifth pane, which the "+ Add tool" menu states
// instead of silently doing nothing.
//
// STATE — module scope on purpose. The drawer unmounts when it is collapsed (the
// parent keeps only `toolsOpen`), so a component-local `ref` would throw the whole
// layout away every time the panel is hidden. The layout survives collapsing, tool
// switches and Settings flips; it is reset only when the ACTIVE WORKSPACE changes,
// because panes are pointed at one workspace's folders and a shell belongs to the
// tree it was opened in. Nothing is persisted (no `settings.json` key), as before.
//
// SINGLE WRITER — every mutation of the tree goes through a function here: the
// host renders `dockTree` and calls `addTool`, `splitPane`, `closePane`,
// `setContextPath`. No component builds a `Pane` by hand, so there is exactly one
// place that can break the depth cap, the pane cap or the singleton rule.
import { ref } from "vue";
import type { WorkspaceInfo } from "@/contracts/types";
import {
  toolById,
  type ToolContext,
  type ToolId,
  type ToolInstance,
} from "./registry";

/** Splits above a leaf: 0 is the root pane, 2 is the deepest pane. */
export const MAX_DEPTH = 2;

/** At most 4 panes, which is the whole tree depth 2 can hold. */
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

/**
 * The dock of the whole app session (see the module note on why it lives here).
 * `null` means "no pane open", which is the empty face and not an error.
 */
export const dockTree = ref<Pane | null>(null);

/**
 * The pane a new pane is split from, and the one drawn as active. Kept as an id
 * and not as a reference: the tree is replaced on every change, so a stored node
 * would go stale.
 */
export const focusedId = ref<string | null>(null);

/** The workspace the current layout was built for (see `applyDock`). */
const layoutWorkspace = ref<string | null>(null);

/** Instance ids are local and monotonic: stable keys for Vue, nothing durable. */
let sequence = 0;

function nextInstanceId(): string {
  sequence += 1;
  return `pane-${sequence}`;
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

/** Every instance in the tree, left to right, in render order. */
export function instances(tree: Pane | null): ToolInstance[] {
  return leafSlots(tree).map((slot) => slot.instance);
}

/** One instance by id, or `undefined` when the tree no longer holds it. */
export function instanceById(
  tree: Pane | null,
  id: string,
): ToolInstance | undefined {
  return instances(tree).find((instance) => instance.id === id);
}

interface LeafSlot {
  instance: ToolInstance;
  /** Splits above this leaf; `0` is the root pane. */
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

/** The pane a new pane is split from: the focused one, else the shallowest. */
function insertionTarget(tree: Pane | null): string | null {
  const slots = leafSlots(tree);
  const splittable = slots.filter((slot) => slot.depth < MAX_DEPTH);
  const focused = splittable.find((slot) => slot.instance.id === focusedId.value);
  if (focused !== undefined) return focused.instance.id;
  if (splittable.length === 0) return null;
  const shallowest = Math.min(...splittable.map((slot) => slot.depth));
  return splittable.find((slot) => slot.depth === shallowest)?.instance.id ?? null;
}

/** Whether `addTool` would focus an open pane, open a new one, or hit the cap. */
export type AddPreview = "focus" | "open" | "full";

/**
 * What a click on one registry entry would do, without doing it. The "+ Add tool"
 * menu disables the entries that answer `full` and shows the cap, instead of
 * offering an action that would be silently ignored.
 */
export function addPreview(toolId: ToolId): AddPreview {
  const definition = toolById(toolId);
  if (definition === undefined) return "full";
  const open = instances(dockTree.value);
  if (
    definition.singleton === true &&
    open.some((instance) => instance.toolId === toolId)
  ) {
    return "focus";
  }
  if (open.length >= MAX_PANES) return "full";
  return insertionTarget(dockTree.value) === null ? "full" : "open";
}

/**
 * Opens one tool: focuses the open pane when the tool is a singleton, otherwise
 * splits the focused pane to the right (or the shallowest one when the focused
 * pane is already at the depth cap), and reports which happened.
 */
export function addTool(
  toolId: ToolId,
  workspace: WorkspaceInfo | null,
): AddPreview {
  const decision = addPreview(toolId);
  if (decision === "focus") {
    const singleton = instances(dockTree.value).find(
      (instance) => instance.toolId === toolId,
    );
    if (singleton !== undefined) focusedId.value = singleton.id;
    return "focus";
  }
  if (decision === "full") return "full";
  const target = insertionTarget(dockTree.value);
  if (target === null) return "full";
  const instance = makeInstance(toolId, workspace);
  dockTree.value = splitAt(dockTree.value, target, "row", instance);
  focusedId.value = instance.id;
  return "open";
}

/** Replaces the target leaf with a split holding it and `instance`. */
function splitAt(
  tree: Pane | null,
  targetId: string,
  direction: SplitDirection,
  instance: ToolInstance,
): Pane | null {
  const twin: Pane = { kind: "leaf", instance };
  if (tree === null) return twin;
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
 * Splits one pane in two. The new pane shows the same tool with the same context,
 * which is the useful starting point: a second Files pane on the same folder, one
 * "Browse" away from a different one; a second shell in the same directory, one
 * `cd` away from another.
 *
 * Refused — and the buttons are hidden, so it cannot be clicked — when the tool is
 * a singleton, when the pane is already at the depth cap, or when the dock is full.
 */
export function splitPane(
  instanceId: string,
  direction: SplitDirection,
): void {
  const open = instances(dockTree.value);
  if (open.length >= MAX_PANES) return;
  const source = open.find((instance) => instance.id === instanceId);
  if (source === undefined) return;
  if (toolById(source.toolId)?.singleton === true) return;
  const slot = leafSlots(dockTree.value).find(
    (candidate) => candidate.instance.id === instanceId,
  );
  if (slot === undefined || slot.depth >= MAX_DEPTH) return;
  const twin = {
    ...source,
    id: nextInstanceId(),
    context: { ...source.context },
  } as ToolInstance;
  dockTree.value = splitAt(dockTree.value, instanceId, direction, twin);
  focusedId.value = twin.id;
}

/**
 * Closes one pane. The tree collapses around the gap, and the focus moves to the
 * first remaining pane when the closed one had it.
 */
export function closePane(instanceId: string): void {
  if (dockTree.value === null) return;
  dockTree.value = mapLeaves(dockTree.value, (leaf) =>
    leaf.instance.id === instanceId ? null : leaf,
  );
  if (focusedId.value === instanceId) {
    focusedId.value = instances(dockTree.value)[0]?.id ?? null;
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
 * shell) gets its OWN root instead of the workspace's: the context is per
 * instance, and the pane remounts on it because the mount key includes it.
 */
export function setContextPath(instanceId: string, path: string): void {
  if (dockTree.value === null) return;
  const trimmed = path.trim();
  if (trimmed === "") return;
  dockTree.value =
    mapLeaves(dockTree.value, (leaf) => {
      if (leaf.instance.id !== instanceId) return leaf;
      const instance = leaf.instance;
      if (instance.toolId === "files") {
        return {
          kind: "leaf",
          instance: { ...instance, context: { ...instance.context, rootPath: trimmed } },
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
    }) ?? null;
}

/** Marks one pane as the one a split starts from, and as the drawn-active one. */
export function focusPane(instanceId: string): void {
  focusedId.value = instanceId;
}

/** The first enabled tool in registry order, or `null` when nothing is enabled. */
function firstEnabled(enabled: readonly string[]): ToolId | null {
  return toolById(enabled.find((id) => toolById(id) !== undefined) ?? "")?.id ?? null;
}

/** Drops the layout and opens one pane of the first enabled tool. */
function openInitial(workspace: WorkspaceInfo | null, enabled: readonly string[]): void {
  const toolId = firstEnabled(enabled);
  dockTree.value =
    toolId === null ? null : { kind: "leaf", instance: makeInstance(toolId, workspace) };
  focusedId.value = dockTree.value?.kind === "leaf" ? dockTree.value.instance.id : null;
}

/**
 * Reconciles the dock with the two things it depends on. Called by the host on
 * mount and whenever either changes (the drawer being collapsed is not a change:
 * the layout lives at module scope and is still here when it reopens).
 *
 *  1. A new active workspace resets the layout: every pane is pointed at a folder
 *     of the old one, and a shell belongs to the tree it was opened in.
 *  2. A tool the human disabled is closed, and the tree collapses around it
 *     (S5: an off tool is never mounted, which is also what keeps the terminal
 *     from opening a shell nobody asked for).
 *  3. Nothing open but something enabled reopens the first enabled tool, so
 *     turning a tool on shows it instead of leaving an empty dock.
 */
export function applyDock(
  workspace: WorkspaceInfo | null,
  enabled: readonly string[],
): void {
  const key = workspace?.path ?? null;
  if (layoutWorkspace.value !== key) {
    layoutWorkspace.value = key;
    openInitial(workspace, enabled);
    return;
  }
  const available = new Set(enabled);
  if (dockTree.value !== null) {
    dockTree.value = mapLeaves(dockTree.value, (leaf) =>
      available.has(leaf.instance.toolId) ? leaf : null,
    );
  }
  if (instanceById(dockTree.value, focusedId.value ?? "") === undefined) {
    focusedId.value = instances(dockTree.value)[0]?.id ?? null;
  }
  if (dockTree.value === null) openInitial(workspace, enabled);
}
