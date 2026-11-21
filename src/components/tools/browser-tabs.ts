// The Browser pane's internal tab strip
// (`odd/tasks/drawer-responsiveness-oauth.md`, S3 and L3).
//
// L3: a page that asks for a new window gets a tab INSIDE the Browser pane, next
// to the page that opened it — not a drawer tab. The pane therefore owns a small
// tab model, and it lives here rather than in `BrowserTool.vue` because its three
// invariants are cheap to state and hard to keep right inside a component:
//
// 1. one tab per child webview, and a popup is never drawn without the tab that
//    opened it (there would be nowhere to return on close);
// 2. exactly one tab is drawn at a time, and a new one takes the focus — an OAuth
//    popup is the thing the human has to act in;
// 3. a manual close returns to the opener, and the page tab itself is not
//    closable: it is the pane.
//
// Everything here is pure and total: an unknown id, a duplicate view or a popup
// with no opener leaves the state it was given, so the caller can compare by
// identity instead of re-deriving what changed. `tests/browser-tabs.test.ts` pins
// each rule through this contract.

/** One tab: the child webview it draws, and the tab that opened it. */
export interface BrowserTab {
  /** The child webview of the tab: `embed-<n>`. */
  id: string;
  /**
   * `null` for the page tab — the view the human drives and the agent targets —
   * and the opener's id for a popup. A popup closes back to this tab.
   */
  openerId: string | null;
}

/** The strip, in draw order, plus which tab is in front. */
export interface BrowserTabState {
  tabs: BrowserTab[];
  activeId: string | null;
}

/** No tab at all: the state before the page view exists. */
export function emptyTabs(): BrowserTabState {
  return { tabs: [], activeId: null };
}

/** Whether a tab is a popup, i.e. something the human closes by hand. */
export function isPopupTab(tab: BrowserTab): boolean {
  return tab.openerId !== null;
}

/** One tab by id, or `null`. */
export function tabById(state: BrowserTabState, id: string): BrowserTab | null {
  return state.tabs.find((tab) => tab.id === id) ?? null;
}

/** The tab in front, or `null` when the strip is empty. */
export function activeTab(state: BrowserTabState): BrowserTab | null {
  return state.activeId === null ? null : tabById(state, state.activeId);
}

/** The answer of [`openTab`]: the new state, and whether the tab is in it. */
export interface OpenTabResult {
  state: BrowserTabState;
  opened: boolean;
}

/**
 * Adds a tab and draws it.
 *
 * A popup is inserted right after its opener, so the strip reads in the order the
 * windows were opened; the page tab goes last (it is normally the only one). A
 * duplicate id, an empty id, or a popup whose opener is not a tab changes
 * nothing: `opened === false` is the caller's signal to close that native view
 * instead of drawing a tab nobody can return from.
 */
export function openTab(
  state: BrowserTabState,
  tab: BrowserTab,
): OpenTabResult {
  if (tab.id === "" || tabById(state, tab.id) !== null) {
    return { state, opened: false };
  }
  if (tab.openerId !== null && tabById(state, tab.openerId) === null) {
    return { state, opened: false };
  }
  const tabs = [...state.tabs];
  const openerIndex =
    tab.openerId === null
      ? -1
      : tabs.findIndex((candidate) => candidate.id === tab.openerId);
  if (openerIndex === -1) {
    tabs.push(tab);
  } else {
    tabs.splice(openerIndex + 1, 0, tab);
  }
  return { state: { tabs, activeId: tab.id }, opened: true };
}

/**
 * Draws another tab. An unknown id, or the tab already in front, answers the state
 * it was given.
 */
export function activateTab(
  state: BrowserTabState,
  id: string,
): BrowserTabState {
  if (state.activeId === id || tabById(state, id) === null) return state;
  return { tabs: state.tabs, activeId: id };
}

/** The answer of a close: the new state, and the views the caller must close. */
export interface CloseTabResult {
  state: BrowserTabState;
  /**
   * Every view that left the strip, the asked-for tab first. The caller closes
   * exactly these native webviews, in this order.
   */
  closed: string[];
}

/**
 * Closes one popup tab, and every popup it opened.
 *
 * The page tab is refused (`closed: []`): it is the pane, so it has no close
 * button and no model path that removes it. A closed tab that was in front hands
 * the focus to its opener — L3's "manual close returns to the opener tab" — which
 * always survives, because a close only ever removes popups.
 */
export function closeTab(state: BrowserTabState, id: string): CloseTabResult {
  const tab = tabById(state, id);
  if (tab === null || tab.openerId === null) {
    return { state, closed: [] };
  }
  const closed = [id, ...descendants(state, id)];
  const gone = new Set(closed);
  const tabs = state.tabs.filter((candidate) => !gone.has(candidate.id));
  const activeId =
    state.activeId === null || !gone.has(state.activeId)
      ? state.activeId
      : neighbour(state, tab, tabs);
  return { state: { tabs, activeId }, closed };
}

/**
 * Closes every popup and leaves the page tab in front.
 *
 * The pane uses this when the page view it holds is rebuilt: the popups were
 * opened from a view that no longer exists. Order is the strip's own, so the
 * deep tabs go last.
 */
export function closeAllPopups(state: BrowserTabState): CloseTabResult {
  const closed = state.tabs.filter(isPopupTab).map((tab) => tab.id);
  if (closed.length === 0) return { state, closed: [] };
  const gone = new Set(closed);
  const tabs = state.tabs.filter((tab) => !gone.has(tab.id));
  const activeId =
    state.activeId !== null && !gone.has(state.activeId)
      ? state.activeId
      : (tabs[0]?.id ?? null);
  return { state: { tabs, activeId }, closed };
}

/** Every popup opened by `id`, transitively, in strip order. */
function descendants(state: BrowserTabState, id: string): string[] {
  const found: string[] = [];
  let frontier = [id];
  while (frontier.length > 0) {
    const next = state.tabs
      .filter(
        (tab) =>
          tab.openerId !== null &&
          frontier.includes(tab.openerId) &&
          !found.includes(tab.id),
      )
      .map((tab) => tab.id);
    found.push(...next);
    frontier = next;
  }
  return found;
}

/**
 * Which tab takes the focus after a close: the closed tab's opener when it
 * survives, otherwise the nearest tab before it and then after it, and `null`
 * when nothing is left.
 */
function neighbour(
  state: BrowserTabState,
  closedTab: BrowserTab,
  remaining: BrowserTab[],
): string | null {
  const survives = (id: string | null): id is string =>
    id !== null && remaining.some((tab) => tab.id === id);
  if (survives(closedTab.openerId)) return closedTab.openerId;
  const index = state.tabs.findIndex((tab) => tab.id === closedTab.id);
  for (let before = index - 1; before >= 0; before -= 1) {
    const candidate = state.tabs[before];
    if (candidate !== undefined && survives(candidate.id)) return candidate.id;
  }
  for (let after = index + 1; after < state.tabs.length; after += 1) {
    const candidate = state.tabs[after];
    if (candidate !== undefined && survives(candidate.id)) return candidate.id;
  }
  return remaining[0]?.id ?? null;
}

/** One tab as the strip draws it: presentation only, derived from the model. */
export interface BrowserTabStripItem {
  id: string;
  /** The visible text; the pane decides it (the page's label, a popup's host). */
  label: string;
  popup: boolean;
  /** The tab in front: the one the child webviews are placed for. */
  active: boolean;
  /** Only a popup can be closed by hand. */
  closable: boolean;
}

/**
 * What the strip draws, in order. `labelOf` is the pane's own decision — the
 * model never invents a name for a page it cannot see.
 */
export function stripItems(
  state: BrowserTabState,
  labelOf: (tab: BrowserTab) => string,
): BrowserTabStripItem[] {
  return state.tabs.map((tab) => ({
    id: tab.id,
    label: labelOf(tab),
    popup: isPopupTab(tab),
    active: tab.id === state.activeId,
    closable: isPopupTab(tab),
  }));
}
