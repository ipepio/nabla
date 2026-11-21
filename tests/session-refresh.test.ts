// The drawer's narrow refresh: which open list a change re-reads, and the one
// snapshot write a finished turn is worth (S2 of
// `odd/tasks/drawer-responsiveness-oauth.md`).
//
// Both are policy, not plumbing, so both are exercised here through the module
// the drawer and the store use: the planner is created per test (its state is
// private), and the preview write is observed through the callback the caller
// supplies — a write that happened is a write the caller can see, which is
// exactly the waste being removed.
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  createSessionRefreshPlanner,
  forgetSavedPreview,
  savePreviewOnce,
  sessionRefresh,
} from "../src/lib/session-refresh.ts";
import type { PreviewSnapshot } from "../src/lib/session-refresh.ts";

/** One turn as the store snapshots it: role, text, reasoning and time. */
const TURN: PreviewSnapshot[] = [
  { role: "user", text: "review the diff", reasoning: null, at: "2026-07-01T10:00:00Z" },
  {
    role: "assistant",
    text: "nothing to fix",
    reasoning: "looked at the patch",
    at: "2026-07-01T10:00:01Z",
  },
];

// ── which open list a change re-reads ────────────────────────────────────────

test("a finished turn re-reads only the workspace it happened in", () => {
  const planner = createSessionRefreshPlanner();

  planner.note(7);

  const plan = planner.takePlan([5, 7]);
  assert.deepEqual(plan.workspaces, [7]);
  assert.equal(plan.all, false, "an attributed change is never a global refresh");
});

test("the same change is never read twice", () => {
  const planner = createSessionRefreshPlanner();
  planner.note(7);

  assert.deepEqual(planner.takePlan([7]).workspaces, [7]);
  // Nothing new happened: a second tick (a poll, a re-render) must not re-read.
  assert.deepEqual(planner.takePlan([7]).workspaces, []);
});

test("nothing noted, nothing read", () => {
  const planner = createSessionRefreshPlanner();

  const plan = planner.takePlan([3, 4]);
  assert.deepEqual(plan.workspaces, []);
  assert.equal(plan.all, false);
});

test("a change in a closed list costs no read, now or when it reopens", () => {
  const planner = createSessionRefreshPlanner();

  // Workspace 5 is collapsed: it has no rows to refresh.
  planner.note(5);
  assert.deepEqual(planner.takePlan([7]).workspaces, []);

  // Expanding 5 reads it anyway (`syncSessions`), so the plan must not queue a
  // second read of a list that was refreshed on its own.
  assert.deepEqual(planner.takePlan([5, 7]).workspaces, []);
});

test("a change the store cannot attribute re-reads every open list", () => {
  const planner = createSessionRefreshPlanner();

  // An event for a session the store never placed in a workspace: the change is
  // real but unaddressed, so the narrow path cannot be narrower than "all open".
  planner.note(null);

  const plan = planner.takePlan([3, 4]);
  assert.equal(plan.all, true);
  assert.deepEqual(plan.workspaces, [3, 4]);
});

test("a removed workspace forgets its activity", () => {
  const planner = createSessionRefreshPlanner();
  planner.note(9);
  assert.equal(planner.revision(9), 1);

  planner.forget(9);

  assert.equal(planner.revision(9), 0);
  assert.deepEqual(planner.takePlan([9]).workspaces, []);
});

test("every noted change bumps the one signal a view watches", () => {
  const planner = createSessionRefreshPlanner();
  const before = planner.signal.value;

  planner.note(1);
  planner.note(null);

  assert.equal(planner.signal.value, before + 2);
});

test("the app plans from one shared instance", () => {
  // The store notes on it and the drawer reads it; both must be the same object,
  // or a finished turn would move a counter nobody is watching.
  assert.equal(typeof sessionRefresh.note, "function");
  assert.equal(typeof sessionRefresh.takePlan, "function");
  assert.equal(typeof sessionRefresh.signal.value, "number");
});

// ── one snapshot write per finished turn ─────────────────────────────────────

/** A second finished turn: the conversation moved one message further. */
const NEXT_TURN: PreviewSnapshot[] = [
  ...TURN,
  {
    role: "assistant",
    text: "and it is ready",
    reasoning: null,
    at: "2026-07-01T10:00:09Z",
  },
];

/** A write the test decides the fate of, so two asks can overlap for certain. */
function deferred(): { promise: Promise<boolean>; resolve: (stored: boolean) => void } {
  let resolve!: (stored: boolean) => void;
  const promise = new Promise<boolean>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

/** Lets every scheduled microtask run, so a queued write can reach the bridge. */
function flush(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

/** Which tail a write carried: the last message's text names it. */
function tailOf(messages: readonly PreviewSnapshot[]): string {
  return messages[messages.length - 1].text;
}

test("a turn that closes twice snapshots its preview once", async () => {
  forgetSavedPreview("closes-twice");
  const writes: PreviewSnapshot[][] = [];
  const write = async (messages: PreviewSnapshot[]): Promise<boolean> => {
    writes.push(messages);
    return true;
  };

  // `turn_end` and then `agent_end` close the same turn: same buffer, same tail.
  assert.equal(await savePreviewOnce("closes-twice", TURN, write), true);
  assert.equal(await savePreviewOnce("closes-twice", TURN, write), false);

  assert.equal(writes.length, 1);
  assert.deepEqual(writes[0], TURN, "the tail is written whole, not truncated");
});

test("the next finished turn is still written", async () => {
  forgetSavedPreview("next-turn");
  let writes = 0;
  const write = async (): Promise<boolean> => {
    writes += 1;
    return true;
  };

  await savePreviewOnce("next-turn", TURN, write);
  const answered = [
    ...TURN,
    {
      role: "assistant",
      text: "and it is ready",
      reasoning: null,
      at: "2026-07-01T10:00:09Z",
    },
  ];
  assert.equal(await savePreviewOnce("next-turn", answered, write), true);

  assert.equal(writes, 2);
});

test("a tail that only gained reasoning is a different snapshot", async () => {
  forgetSavedPreview("rethought");
  let writes = 0;
  const write = async (): Promise<boolean> => {
    writes += 1;
    return true;
  };

  // Reasoning is the part pi's JSONL never stores — the whole reason the preview
  // exists — so it takes part in "is this the same tail".
  const rethought = [TURN[0], { ...TURN[1], reasoning: "looked twice" }];
  await savePreviewOnce("rethought", TURN, write);
  assert.equal(await savePreviewOnce("rethought", rethought, write), true);

  assert.equal(writes, 2);
});

test("a write that did not land is not remembered as saved", async () => {
  forgetSavedPreview("not-stored");
  // Rust answers `false` for a session it could not store (or a failed store).
  assert.equal(await savePreviewOnce("not-stored", TURN, async () => false), false);

  let writes = 0;
  const write = async (): Promise<boolean> => {
    writes += 1;
    return true;
  };
  assert.equal(await savePreviewOnce("not-stored", TURN, write), true);
  assert.equal(writes, 1, "the retry really wrote, so nothing was dropped");
});

test("a failing write is retried instead of remembered", async () => {
  forgetSavedPreview("rejects");
  const reject = async (): Promise<boolean> => {
    throw new Error("bridge down");
  };

  // Best effort by contract: the failure is swallowed, never thrown at the turn
  // that produced the snapshot.
  assert.equal(await savePreviewOnce("rejects", TURN, reject), false);

  let writes = 0;
  const write = async (): Promise<boolean> => {
    writes += 1;
    return true;
  };
  assert.equal(await savePreviewOnce("rejects", TURN, write), true);
  assert.equal(writes, 1);
});

test("a deleted session forgets its preview memory", async () => {
  forgetSavedPreview("deleted");
  let writes = 0;
  const write = async (): Promise<boolean> => {
    writes += 1;
    return true;
  };

  await savePreviewOnce("deleted", TURN, write);
  forgetSavedPreview("deleted");
  assert.equal(await savePreviewOnce("deleted", TURN, write), true);

  assert.equal(writes, 2);
});

// ── asks that overlap: nothing has landed yet when the next one arrives ─────

test("a second close before the first write lands is not a second write", async () => {
  forgetSavedPreview("overlapping-closes");
  const landed = deferred();
  let writes = 0;
  const write = (): Promise<boolean> => {
    writes += 1;
    return landed.promise;
  };

  // `turn_end` and `agent_end` arrive back to back: the first IPC is still on its
  // way, so "what is stored" cannot answer the second ask.
  const first = savePreviewOnce("overlapping-closes", TURN, write);
  const second = savePreviewOnce("overlapping-closes", TURN, write);
  await flush();
  assert.equal(writes, 1, "the second ask joined the write already on its way");

  landed.resolve(true);
  assert.equal(await first, true);
  assert.equal(await second, true);
  assert.equal(writes, 1);
});

test("overlapping tails are written in order and the newest one is what stays", async () => {
  forgetSavedPreview("back-to-back-tails");
  const gates = [deferred(), deferred(), deferred()];
  const order: string[] = [];
  const write = (messages: PreviewSnapshot[]): Promise<boolean> => {
    order.push(tailOf(messages));
    return gates[order.length - 1].promise;
  };

  // A → B → A: the last tail is A again, so the row must end on A.
  const first = savePreviewOnce("back-to-back-tails", TURN, write);
  const second = savePreviewOnce("back-to-back-tails", NEXT_TURN, write);
  const third = savePreviewOnce("back-to-back-tails", TURN, write);

  await flush();
  assert.deepEqual(order, [tailOf(TURN)], "a newer tail waits for the one on its way");

  gates[0].resolve(true);
  await flush();
  assert.deepEqual(order, [tailOf(TURN), tailOf(NEXT_TURN)]);

  gates[1].resolve(true);
  await flush();
  assert.deepEqual(order, [tailOf(TURN), tailOf(NEXT_TURN), tailOf(TURN)]);

  gates[2].resolve(true);
  assert.deepEqual(
    [await first, await second, await third],
    [true, true, true],
    "each ask answers for the write it queued",
  );

  // A → B → A ends on A, so A is what is remembered and asking again is free.
  let extra = 0;
  assert.equal(
    await savePreviewOnce("back-to-back-tails", TURN, async () => {
      extra += 1;
      return true;
    }),
    false,
  );
  assert.equal(extra, 0, "the newest tail is the one remembered");
});

test("a stored tail is written again when a newer one is on its way back to it", async () => {
  forgetSavedPreview("superseded");
  assert.equal(
    await savePreviewOnce("superseded", TURN, async () => true),
    true,
    "A is stored",
  );

  const gate = deferred();
  const order: string[] = [];
  const write = (messages: PreviewSnapshot[]): Promise<boolean> => {
    order.push(tailOf(messages));
    return gate.promise;
  };

  const newer = savePreviewOnce("superseded", NEXT_TURN, write);
  // The stored A no longer describes the row: B is on its way and will land
  // after it, so A has to be queued again rather than skipped as "already there".
  const back = savePreviewOnce("superseded", TURN, write);

  await flush();
  assert.deepEqual(order, [tailOf(NEXT_TURN)]);
  gate.resolve(true);

  assert.equal(await newer, true);
  assert.equal(await back, true);
  assert.deepEqual(order, [tailOf(NEXT_TURN), tailOf(TURN)]);
});

test("a write that fails with an ask waiting is retried by the next turn", async () => {
  forgetSavedPreview("overlapping-failure");
  let writes = 0;
  const failing = async (): Promise<boolean> => {
    writes += 1;
    return false;
  };

	// Both asks overlap on one attempt, which fails; nothing is remembered.
  const first = savePreviewOnce("overlapping-failure", TURN, failing);
  const second = savePreviewOnce("overlapping-failure", TURN, failing);
  assert.deepEqual([await first, await second], [false, false]);
  assert.equal(writes, 1, "a failed write is not repeated for the same tail");

  let stored = 0;
  assert.equal(
    await savePreviewOnce("overlapping-failure", TURN, async () => {
      stored += 1;
      return true;
    }),
    true,
    "nothing was remembered, so the next close really writes",
  );
  assert.equal(stored, 1);
});

test("a deleted session drops the asks still waiting and remembers nothing", async () => {
  forgetSavedPreview("deleted-mid-write");
  const gate = deferred();
  const written: string[] = [];
  const write = (messages: PreviewSnapshot[]): Promise<boolean> => {
    written.push(tailOf(messages));
    return gate.promise;
  };

  const onItsWay = savePreviewOnce("deleted-mid-write", TURN, write);
  const queued = savePreviewOnce("deleted-mid-write", NEXT_TURN, write);
  await flush();
  assert.deepEqual(written, [tailOf(TURN)], "the second tail waits");

  // The row is deleted while its first write is still on its way.
  forgetSavedPreview("deleted-mid-write");
  gate.resolve(true);
  await onItsWay;
  assert.equal(await queued, false, "an ask for a deleted row is dropped");
  assert.deepEqual(written, [tailOf(TURN)], "and it never reached the bridge");

  // Nothing is remembered: the same tail is new again and must be written.
  let rewritten = 0;
  assert.equal(
    await savePreviewOnce("deleted-mid-write", TURN, async () => {
      rewritten += 1;
      return true;
    }),
    true,
  );
  assert.equal(rewritten, 1);
});

test("sessions never share snapshot memory", async () => {
  forgetSavedPreview("one");
  forgetSavedPreview("two");
  const written: string[] = [];
  const writeFor = (id: string) => async (): Promise<boolean> => {
    written.push(id);
    return true;
  };

  await savePreviewOnce("one", TURN, writeFor("one"));
  // The same tail in another session is that session's first snapshot.
  assert.equal(await savePreviewOnce("two", TURN, writeFor("two")), true);

  assert.deepEqual(written, ["one", "two"]);
});
