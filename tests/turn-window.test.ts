// Behaviour of the transcript's turn window
// (`src/components/chat/turn-window.ts`).
//
// The window is what keeps a long conversation from mounting every turn it has
// ever had: it mounts the newest page, reveals older pages on request, and never
// drops what it already revealed. A different conversation starts over.
import { test } from "node:test";
import assert from "node:assert/strict";
import { nextTick, ref } from "vue";

import { useTurnWindow } from "../src/components/chat/turn-window.ts";

interface Group {
  id: string;
}

function groups(count: number, from = 0): Group[] {
  return Array.from({ length: count }, (_, index) => ({ id: `g${from + index}` }));
}

function ids(list: readonly Group[]): string[] {
  return list.map((group) => group.id);
}

function windowOf(count: number, identity: () => string | null = () => "s1") {
  const list = ref<Group[]>(groups(count));
  const window = useTurnWindow(list, identity, { initial: 12, page: 12 });
  return { list, window };
}

test("a long conversation mounts only its newest page", () => {
  const { window } = windowOf(40);
  assert.equal(window.groups.value.length, 12);
  assert.deepEqual(ids(window.groups.value), ids(groups(12, 28)));
  assert.equal(window.hiddenCount.value, 28);
  assert.equal(window.hasOlder.value, true);
});

test("a conversation shorter than a page is untouched", () => {
  const { window } = windowOf(5);
  assert.deepEqual(ids(window.groups.value), ids(groups(5)));
  assert.equal(window.hiddenCount.value, 0);
  assert.equal(window.hasOlder.value, false);
});

test("an empty conversation has nothing to reveal", () => {
  const { window } = windowOf(0);
  assert.deepEqual(window.groups.value, []);
  assert.equal(window.hiddenCount.value, 0);
  assert.equal(window.hasOlder.value, false);
  window.showOlder();
  assert.deepEqual(window.groups.value, []);
});

test("revealing walks back one page at a time and stops at the first turn", () => {
  const { window } = windowOf(40);
  window.showOlder();
  assert.equal(window.hiddenCount.value, 16);
  assert.deepEqual(ids(window.groups.value), ids(groups(24, 16)));
  window.showOlder();
  assert.equal(window.hiddenCount.value, 4);
  window.showOlder();
  assert.equal(window.hiddenCount.value, 0);
  assert.deepEqual(ids(window.groups.value), ids(groups(40)));
  assert.equal(window.hasOlder.value, false);
  // Past the first turn the control is inert instead of going negative.
  window.showOlder();
  assert.equal(window.hiddenCount.value, 0);
  assert.equal(window.groups.value.length, 40);
});

test("a revealed window keeps what it already mounted while new turns arrive", async () => {
  const { list, window } = windowOf(40);
  window.showOlder();
  const revealed = ids(window.groups.value);
  list.value = [...list.value, ...groups(3, 40)];
  await nextTick();
  assert.deepEqual(ids(window.groups.value).slice(0, revealed.length), revealed);
  assert.equal(window.groups.value.at(-1)?.id, "g42");
});

test("history that arrives after mount is bounded too", async () => {
  const list = ref<Group[]>([]);
  const window = useTurnWindow(list, () => "s1", { initial: 12, page: 12 });
  assert.equal(window.hiddenCount.value, 0);
  list.value = groups(30);
  await nextTick();
  assert.equal(window.hiddenCount.value, 18);
  assert.equal(window.groups.value.length, 12);
});

test("another conversation starts from its own newest page", async () => {
  const identity = ref("s1");
  const { list, window } = windowOf(40, () => identity.value);
  window.showOlder();
  window.showOlder();
  assert.equal(window.hiddenCount.value, 4);
  identity.value = "s2";
  list.value = groups(30, 100);
  await nextTick();
  assert.equal(window.hiddenCount.value, 18);
  assert.deepEqual(ids(window.groups.value), ids(groups(12, 118)));
});

test("a transcript that shrinks never asks for turns that are gone", async () => {
  const { list, window } = windowOf(40);
  window.showOlder();
  window.showOlder();
  window.showOlder();
  assert.equal(window.hiddenCount.value, 0);
  list.value = groups(3);
  await nextTick();
  assert.deepEqual(ids(window.groups.value), ids(groups(3)));
  assert.equal(window.hiddenCount.value, 0);
});

test("the live turn is always inside the window", async () => {
  const { list, window } = windowOf(40);
  list.value = groups(41, 0);
  await nextTick();
  assert.equal(window.groups.value.at(-1)?.id, "g40");
});
