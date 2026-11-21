// Behaviour of the transcript's follow controller
// (`src/components/chat/transcript-follow.ts`).
//
// The controller is the only thing that moves a transcript's scroll offset, so
// these tests pin the three promises the views rely on: repeated requests are
// coalesced into one scroll per frame, a reader who scrolled up is never pulled
// back, and nothing survives the element it was bound to.
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  BOTTOM_SLACK_PX,
  createTranscriptFollow,
} from "../src/components/chat/transcript-follow.ts";

interface FakeScroller {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

function scroller(init: Partial<FakeScroller> = {}): FakeScroller {
  return {
    scrollTop: init.scrollTop ?? 0,
    scrollHeight: init.scrollHeight ?? 1000,
    clientHeight: init.clientHeight ?? 500,
  };
}

/** A frame scheduler a test drives by hand: nothing moves until `flush`. */
function frames() {
  const queue: { id: number; run: () => void }[] = [];
  const cancelled: number[] = [];
  let nextId = 1;
  return {
    requestFrame(callback: () => void): number {
      const id = nextId;
      nextId += 1;
      queue.push({ id, run: callback });
      return id;
    },
    cancelFrame(handle: number): void {
      cancelled.push(handle);
      const at = queue.findIndex((frame) => frame.id === handle);
      if (at !== -1) queue.splice(at, 1);
    },
    /** Runs the frames waiting for the next paint, as the browser would. */
    flush(): void {
      const due = queue.splice(0, queue.length);
      for (const frame of due) frame.run();
    },
    waiting(): number {
      return queue.length;
    },
    cancelledIds(): number[] {
      return [...cancelled];
    },
  };
}

function follow(viewport: FakeScroller) {
  const scheduler = frames();
  const controller = createTranscriptFollow({
    requestFrame: (callback) => scheduler.requestFrame(callback),
    cancelFrame: (handle) => scheduler.cancelFrame(handle),
  });
  controller.attach(viewport);
  return { controller, viewport, scheduler };
}

test("opening a transcript lands at the end", () => {
  const { viewport } = follow(scroller({ scrollTop: 0, scrollHeight: 1000 }));
  assert.equal(viewport.scrollTop, 1000);
});

test("repeated requests collapse into one scroll per frame", () => {
  const { controller, viewport, scheduler } = follow(scroller());
  viewport.scrollHeight = 2000;
  controller.request();
  controller.request();
  controller.request();
  assert.equal(scheduler.waiting(), 1);
  assert.equal(viewport.scrollTop, 1000, "nothing moves before the frame");
  scheduler.flush();
  assert.equal(viewport.scrollTop, 2000);
  assert.equal(scheduler.waiting(), 0);
  // The next growth arms a new frame: coalescing is per frame, not one shot.
  viewport.scrollHeight = 2400;
  controller.request();
  assert.equal(scheduler.waiting(), 1);
  scheduler.flush();
  assert.equal(viewport.scrollTop, 2400);
});

test("a reader who scrolled up keeps their place when the frame lands", () => {
  const { controller, viewport, scheduler } = follow(scroller({ scrollTop: 1000, scrollHeight: 1000 }));
  viewport.scrollHeight = 3000;
  controller.request();
  // The human moves up before the frame runs: the request must not drag them.
  viewport.scrollTop = 200;
  controller.handleScroll();
  assert.equal(controller.pinned(), false);
  scheduler.flush();
  assert.equal(viewport.scrollTop, 200);
});

test("scrolling back to the end re-pins and the view follows again", () => {
  const { controller, viewport, scheduler } = follow(scroller());
  viewport.scrollHeight = 3000;
  viewport.scrollTop = 100;
  controller.handleScroll();
  assert.equal(controller.pinned(), false);
  // Back at the end, inside the slack.
  viewport.scrollTop = 3000 - 500 - (BOTTOM_SLACK_PX - 1);
  controller.handleScroll();
  assert.equal(controller.pinned(), true);
  viewport.scrollHeight = 3400;
  controller.request();
  scheduler.flush();
  assert.equal(viewport.scrollTop, 3400);
});

test("a pinned view re-asserts the end without unpinning", () => {
  const { controller, viewport, scheduler } = follow(scroller({ scrollTop: 1000, scrollHeight: 1000 }));
  // The content grew after the programmatic scroll, and the event the browser
  // delivers still reports the offset this controller itself set.
  viewport.scrollHeight = 1500;
  viewport.scrollTop = 1000;
  controller.handleScroll();
  assert.equal(controller.pinned(), true);
  assert.equal(scheduler.waiting(), 1);
  scheduler.flush();
  assert.equal(viewport.scrollTop, 1500);
  assert.equal(controller.pinned(), true);
});

test("jumping scrolls now, pins, and drops a pending frame", () => {
  const { controller, viewport, scheduler } = follow(scroller());
  viewport.scrollHeight = 2000;
  controller.request();
  viewport.scrollHeight = 2600;
  controller.jump();
  assert.equal(viewport.scrollTop, 2600);
  assert.equal(scheduler.waiting(), 0, "the pending frame was cancelled");
  assert.deepEqual(scheduler.cancelledIds(), [1]);
  scheduler.flush();
  assert.equal(viewport.scrollTop, 2600);
});

test("a pinned resize keeps the view glued; a free one only re-reads the pin", () => {
  const { controller, viewport } = follow(scroller());
  viewport.scrollHeight = 1400;
  controller.handleResize();
  assert.equal(viewport.scrollTop, 1400);

  viewport.scrollTop = 100;
  controller.handleScroll();
  assert.equal(controller.pinned(), false);
  viewport.scrollHeight = 900;
  controller.handleResize();
  assert.equal(viewport.scrollTop, 100, "a free view is never moved by a resize");
  assert.equal(controller.pinned(), false);

  // Content that shrinks into the viewport puts the reader at the end.
  viewport.scrollHeight = 550;
  controller.handleResize();
  assert.equal(controller.pinned(), true);
  assert.equal(viewport.scrollTop, 100);
});

test("detaching cancels the frame and never scrolls a dead element", () => {
  const { controller, viewport, scheduler } = follow(scroller());
  viewport.scrollHeight = 2000;
  controller.request();
  controller.detach();
  assert.equal(scheduler.waiting(), 0);
  assert.deepEqual(scheduler.cancelledIds(), [1]);
  viewport.scrollHeight = 4000;
  scheduler.flush();
  assert.equal(viewport.scrollTop, 1000, "the dropped element was not touched");
  // A request after detach arms nothing: there is nothing to follow.
  controller.request();
  assert.equal(scheduler.waiting(), 0);
});

test("an anchor keeps the reader's place when older turns are prepended", () => {
  const { controller, viewport } = follow(scroller({ scrollTop: 200, scrollHeight: 1000 }));
  viewport.scrollTop = 200;
  const anchor = controller.snapshot();
  assert.deepEqual(anchor, { scrollTop: 200, scrollHeight: 1000 });
  // Prepending a page of older turns grows the content above the reader.
  viewport.scrollHeight = 1500;
  controller.restore(anchor);
  assert.equal(viewport.scrollTop, 700);
});

test("restoring without a snapshot or an element is inert", () => {
  const { controller, viewport } = follow(scroller({ scrollTop: 120, scrollHeight: 1000 }));
  controller.restore(null);
  assert.equal(viewport.scrollTop, 1000);
  viewport.scrollTop = 120;
  controller.restore(controller.snapshot());
  assert.equal(viewport.scrollTop, 120, "an unchanged height moves nothing");
  controller.detach();
  assert.equal(controller.snapshot(), null);
  viewport.scrollHeight = 3000;
  controller.restore({ scrollTop: 120, scrollHeight: 1000 });
  assert.equal(viewport.scrollTop, 120, "a detached controller restores nothing");
});

test("syncPin re-reads the position without moving the view", () => {
  const { controller, viewport } = follow(scroller());
  viewport.scrollTop = 120;
  viewport.scrollHeight = 2000;
  controller.syncPin();
  assert.equal(controller.pinned(), false);
  assert.equal(viewport.scrollTop, 120);
  viewport.scrollTop = 1500;
  controller.syncPin();
  assert.equal(controller.pinned(), true);
  assert.equal(viewport.scrollTop, 1500);
});
