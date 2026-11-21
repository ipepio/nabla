// The Browser pane's internal tab strip
// (`odd/tasks/drawer-responsiveness-oauth.md`, S3 and L3).
//
// L3 is the reason this model exists: a popup opens a tab **inside the Browser
// pane**, next to the page that opened it — not a drawer tab. So the strip has
// exactly three rules to keep, and each one is a place a component would quietly
// get it wrong:
//
// 1. one tab per child webview, and a popup never appears without the page that
//    opened it (there would be nowhere to return to on close);
// 2. exactly one tab is drawn at a time — the new one takes the focus, because
//    an OAuth popup is what the human has to act in;
// 3. a manual close returns to the opener. The page tab itself is not closable:
//    it IS the pane.
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  activateTab,
  activeTab,
  closeAllPopups,
  closeTab,
  emptyTabs,
  isPopupTab,
  openTab,
  stripItems,
  type BrowserTab,
  type BrowserTabState,
} from "../src/components/tools/browser-tabs.ts";

/** The page the human drives: the pane's own view, the agent's target. */
const PAGE: BrowserTab = { id: "embed-1", openerId: null };
/** A popup the page opened with `window.open`. */
const POPUP: BrowserTab = { id: "embed-2", openerId: "embed-1" };
/** A popup opened by another popup (a provider that opens a second window). */
const NESTED: BrowserTab = { id: "embed-3", openerId: "embed-2" };

/** The state after opening a list of tabs, in order. */
function opened(...tabs: BrowserTab[]): BrowserTabState {
  return tabs.reduce((state, tab) => {
    const result = openTab(state, tab);
    assert.equal(result.opened, true, `${tab.id} must open`);
    return result.state;
  }, emptyTabs());
}

function ids(state: BrowserTabState): string[] {
  return state.tabs.map((tab) => tab.id);
}

// ── the model ──────────────────────────────────────────────────────────────

test("the page tab is the first tab and the drawn one", () => {
  const state = opened(PAGE);
  assert.deepEqual(ids(state), ["embed-1"]);
  assert.equal(state.activeId, "embed-1");
  assert.deepEqual(activeTab(state), PAGE);
  assert.equal(isPopupTab(PAGE), false);
});

test("a popup is a tab of its own, right after the page that opened it", () => {
  const state = opened(PAGE, POPUP);
  assert.deepEqual(ids(state), ["embed-1", "embed-2"]);
  assert.equal(isPopupTab(POPUP), true);
  // … and it is the tab in front: the flow the human started is the one to act on.
  assert.equal(state.activeId, "embed-2");
});

test("a popup whose opener is not a tab is not adopted", () => {
  const result = openTab(emptyTabs(), POPUP);
  assert.equal(result.opened, false);
  // Untouched: the pane closes that view instead of drawing a tab nobody can
  // return from.
  assert.deepEqual(result.state, emptyTabs());
  assert.deepEqual(ids(result.state), []);
});

test("one view is one tab, never two", () => {
  const state = opened(PAGE, POPUP);
  const again = openTab(state, POPUP);
  assert.equal(again.opened, false);
  assert.equal(again.state, state, "a duplicate leaves the state alone");
  assert.deepEqual(ids(again.state), ["embed-1", "embed-2"]);
});

test("activating a tab changes only which one is drawn", () => {
  const state = opened(PAGE, POPUP);
  const page = activateTab(state, "embed-1");
  assert.equal(page.activeId, "embed-1");
  assert.deepEqual(ids(page), ["embed-1", "embed-2"], "the order is the strip");
  assert.equal(activeTab(page)?.id, "embed-1");
});

test("activating a tab that is not there changes nothing", () => {
  const state = opened(PAGE, POPUP);
  assert.equal(activateTab(state, "embed-9"), state);
  assert.equal(activeTab(emptyTabs()), null);
});

// ── closing ────────────────────────────────────────────────────────────────

test("closing the drawn popup returns to the tab that opened it", () => {
  const state = opened(PAGE, POPUP);
  const result = closeTab(state, "embed-2");
  assert.deepEqual(result.closed, ["embed-2"]);
  assert.deepEqual(ids(result.state), ["embed-1"]);
  assert.equal(result.state.activeId, "embed-1", "the human is back on the page");
});

test("closing a popup that is not drawn keeps the drawn one", () => {
  const state = opened(PAGE, POPUP, NESTED);
  const back = activateTab(state, "embed-1");
  const result = closeTab(back, "embed-2");
  // A popup's own popups go with it; the drawn tab is untouched.
  assert.deepEqual(result.closed, ["embed-2", "embed-3"]);
  assert.deepEqual(ids(result.state), ["embed-1"]);
  assert.equal(result.state.activeId, "embed-1");
});

test("closing the page tab is refused: it is the pane", () => {
  const state = opened(PAGE, POPUP);
  const result = closeTab(state, "embed-1");
  assert.deepEqual(result.closed, []);
  assert.equal(result.state, state);
  assert.deepEqual(result.closed, [], "the page tab has no close button either");
});

test("closing a tab that is not there closes nothing", () => {
  const state = opened(PAGE);
  const result = closeTab(state, "embed-9");
  assert.deepEqual(result.closed, []);
  assert.equal(result.state, state);
});

test("closing the page's popups leaves the page drawn", () => {
  const state = opened(PAGE, POPUP, NESTED);
  const result = closeAllPopups(state);
  assert.deepEqual(result.closed, ["embed-2", "embed-3"]);
  assert.deepEqual(ids(result.state), ["embed-1"]);
  assert.equal(result.state.activeId, "embed-1");
  // Nothing to close is not a change.
  const none = closeAllPopups(result.state);
  assert.deepEqual(none.closed, []);
  assert.equal(none.state, result.state);
});

// ── what the strip draws ───────────────────────────────────────────────────

test("the strip marks the drawn tab and offers close only on popups", () => {
  const state = opened(PAGE, POPUP);
  const items = stripItems(state, (tab) => (tab.openerId === null ? "Browser" : "example.com"));
  assert.deepEqual(
    items.map((item) => [item.label, item.popup, item.active, item.closable]),
    [
      ["Browser", false, false, false],
      ["example.com", true, true, true],
    ],
  );
  assert.deepEqual(
    items.map((item) => item.id),
    ids(state),
    "the strip order is the model order",
  );
});
