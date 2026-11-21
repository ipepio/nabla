// The drawer's narrow refresh: which open list a change re-reads, and the one
// snapshot write a finished turn is worth (S2 of
// `odd/tasks/drawer-responsiveness-oauth.md`).
//
// Both are policy, not plumbing, so both live here where they can be stated once
// and exercised without mounting a component:
//
// 1. **A change used to be global.** Any activity — a prompt sent, a turn
//    finished, a session created — bumped one counter, and the drawer answered by
//    dropping every cached list and re-reading every open workspace, so a turn
//    landing anywhere blanked and refetched the rows of all of them. `note()`
//    records *which* workspace changed and `takePlan()` answers exactly the open
//    lists that must be re-read; the caller re-reads into the cache it already
//    has, so rows are replaced when the answer lands and never dropped meanwhile.
//
// 2. **A finished turn snapshotted itself twice.** `turn_end` and `agent_end`
//    both close the same turn and both snapshot the session preview, which is two
//    serialized payloads and two SQLite writes for one fact. `savePreviewOnce()`
//    remembers the tail it last stored per session and skips an identical one —
//    while every turn whose conversation actually moved is still written, and a
//    write that did not land is retried instead of being remembered. The two
//    events arrive back to back, so the second ask usually lands before the first
//    write has answered; that overlap is one write too, and asks for different
//    tails are queued per session so the row ends on the newest one.
//
// Nothing here talks to the bridge: the caller passes its own write, so the
// module owns the decision and the caller keeps the contract.
import { reactive, ref } from "vue";
import type { Ref } from "vue";

/** What one refresh tick tells the drawer to do. */
export interface RefreshPlan {
  /** The open workspaces whose sessions changed: re-read these and nothing else. */
  workspaces: number[];
  /**
   * The change could not be attributed to a workspace (an event for a session the
   * store has never placed), so every open list is re-read. `workspaces` then
   * holds all of them.
   */
  all: boolean;
}

/** The refresh signal a view follows, and the planner behind it. */
export interface SessionRefreshPlanner {
  /**
   * Bumped on every noted change. A view watches this one ref instead of polling
   * the planner, and asks `takePlan` for what to do about it.
   */
  readonly signal: Ref<number>;
  /** Records that something in `workspaceId` changed; `null` when unknown. */
  note(workspaceId: number | null): void;
  /** How many changes `workspaceId` has accumulated; `0` when it never changed. */
  revision(workspaceId: number): number;
  /** What must be re-read since the last plan, given the lists that are open now. */
  takePlan(expandedIds: readonly number[]): RefreshPlan;
  /** Forgets a workspace that no longer exists, so its revisions cannot linger. */
  forget(workspaceId: number): void;
}

/**
 * Creates a planner. The app keeps one (`sessionRefresh`); a test creates its own,
 * because the state a plan is taken against is private and must not leak between
 * cases.
 */
export function createSessionRefreshPlanner(): SessionRefreshPlanner {
  /** Changes per workspace, so a plan can name the ones that moved. */
  const revisions = reactive(new Map<number, number>());
  const signal = ref(0);
  /**
   * What the caller has already been told about. Marked for every known
   * workspace, not only the expanded ones: a list that was closed while its
   * session changed has no rows to refresh and is re-read when it expands, so
   * reporting it would buy a second read of the same answer.
   */
  const seen = new Map<number, number>();
  /** Signal value the last plan was taken at; equal means "nothing new". */
  let seenSignal = 0;
  /** Unattributed changes the caller has already been told about. */
  let unattributed = 0;
  let seenUnattributed = 0;

  return {
    signal,
    note(workspaceId: number | null): void {
      signal.value += 1;
      if (workspaceId === null) {
        unattributed += 1;
        return;
      }
      revisions.set(workspaceId, (revisions.get(workspaceId) ?? 0) + 1);
    },
    revision(workspaceId: number): number {
      return revisions.get(workspaceId) ?? 0;
    },
    takePlan(expandedIds: readonly number[]): RefreshPlan {
      if (signal.value === seenSignal && unattributed === seenUnattributed) {
        return { workspaces: [], all: false };
      }
      seenSignal = signal.value;
      const all = unattributed !== seenUnattributed;
      seenUnattributed = unattributed;
      const expanded = new Set(expandedIds);
      const workspaces: number[] = [];
      for (const workspaceId of expanded) {
        if (all || (revisions.get(workspaceId) ?? 0) !== (seen.get(workspaceId) ?? 0)) {
          workspaces.push(workspaceId);
        }
      }
      for (const [workspaceId, revision] of revisions) {
        seen.set(workspaceId, revision);
      }
      return { workspaces, all };
    },
    forget(workspaceId: number): void {
      revisions.delete(workspaceId);
      seen.delete(workspaceId);
    },
  };
}

/**
 * The one planner the app uses: the store notes on it, the drawer watches it.
 * One instance per app, like every other shared store object.
 */
export const sessionRefresh = createSessionRefreshPlanner();

// ── the session preview snapshot ─────────────────────────────────────────────

/** Enough of one snapshot message to tell two tails apart. */
export interface PreviewSnapshot {
  role: string;
  text: string;
  reasoning: string | null;
  at: string | null;
}

/**
 * The tail last written per session, keyed by session id.
 *
 * Only a write that landed is recorded, so this is knowledge ("that exact
 * snapshot is stored") and never an assumption about a failed one. It also only
 * describes a row nobody is writing to: while a newer tail is on its way, this
 * knowledge is about to stop being true.
 */
const savedPreviews = new Map<string, string>();

/**
 * The writes of one session, one at a time and in the order they were asked for.
 *
 * A preview write is an IPC round trip, so two completions of the same turn ask
 * while the first is still out. Serializing per session is what makes the row
 * end on the newest tail: without it, two overlapping writes can land in either
 * order and an old tail can be the last one stored.
 */
interface PreviewWrites {
  /** Tail of the queue: an ask for it joins instead of writing a second time. */
  tailFingerprint: string | null;
  /** The queued ask's answer; `null` when nothing is queued or in flight. */
  tail: Promise<boolean> | null;
  /** Bumped by `forgetSavedPreview`: asks from an older epoch are dropped. */
  epoch: number;
}

const previewWrites = new Map<string, PreviewWrites>();

function previewWritesFor(key: string): PreviewWrites {
  let writes = previewWrites.get(key);
  if (!writes) {
    writes = { tailFingerprint: null, tail: null, epoch: 0 };
    previewWrites.set(key, writes);
  }
  return writes;
}

/**
 * Every field the contract carries takes part: two tails with the same text but a
 * different reasoning block or a different time are different snapshots, and the
 * reasoning is exactly what pi's own file never stored.
 */
function previewFingerprint(messages: readonly PreviewSnapshot[]): string {
  return JSON.stringify(
    messages.map((message) => [
      message.role,
      message.text,
      message.reasoning,
      message.at,
    ]),
  );
}

/**
 * Forgets a session's snapshot memory: its row is gone, nothing can be re-read.
 *
 * The asks already on their way belong to that gone row too. A write that
 * answers after this must not put its tail back into memory, and an ask still
 * waiting must not reach the bridge at all, or a deleted session would keep
 * costing writes and could shadow a later session that reuses the id.
 */
export function forgetSavedPreview(sessionId: string | number): void {
  const key = String(sessionId);
  savedPreviews.delete(key);
  const writes = previewWrites.get(key);
  if (writes) {
    writes.epoch += 1;
    writes.tail = null;
    writes.tailFingerprint = null;
    previewWrites.delete(key);
  }
}

/**
 * Writes a session's snapshot unless exactly that tail is already stored or on
 * its way, and answers whether this ask's write landed.
 *
 * This is the whole duplicate-completion rule. `turn_end` and `agent_end` both
 * close the same turn, so both ask for a snapshot; the second ask carries the
 * same tail and costs nothing — and it costs nothing even when it arrives before
 * the first write answered, because it joins that write instead of starting a
 * second one.
 *
 * Asks for different tails are queued per session and run in order, so the row
 * ends on the last tail the caller asked for (including A → B → A). Because of
 * that, "already stored" is only trusted while nothing is on its way: with B in
 * flight, a stored A is about to be replaced and has to be written again.
 *
 * The semantics of the snapshot are unchanged: every turn that moved the
 * conversation produces a different tail and is written, and a write that failed
 * (a `false` answer, a rejection) is not remembered, so the next completion
 * retries it.
 *
 * A rejection is swallowed: a preview must never break the turn that produced it.
 */
export async function savePreviewOnce<M extends PreviewSnapshot>(
  sessionId: string | number,
  messages: readonly M[],
  write: (messages: M[]) => Promise<boolean>,
): Promise<boolean> {
  const key = String(sessionId);
  const fingerprint = previewFingerprint(messages);
  const writes = previewWritesFor(key);

  // Coalesce: an ask for the tail already queued is that ask, not a second one.
  if (writes.tail !== null && writes.tailFingerprint === fingerprint) {
    return writes.tail;
  }
  // Only knowledge while the queue is empty; see the note above.
  if (writes.tail === null && savedPreviews.get(key) === fingerprint) return false;

  const epoch = writes.epoch;
  // A copy: the caller keeps its own array (the session buffer) and the write
  // must not be able to see it change while it is on its way.
  const payload = [...messages];
  const queued = (writes.tail ?? Promise.resolve(true)).then(() =>
    savePreview(key, writes, epoch, fingerprint, payload, write),
  );
  writes.tail = queued;
  writes.tailFingerprint = fingerprint;
  try {
    return await queued;
  } finally {
    // The last ask to settle empties the queue; an earlier one leaving its place
    // must not forget the ask still waiting behind it.
    if (writes.tail === queued) {
      writes.tail = null;
      writes.tailFingerprint = null;
    }
  }
}

/**
 * One queued write, run only when every ask before it settled.
 *
 * The epoch is the row's: an ask queued for a session that was deleted meanwhile
 * has nothing to write, and a write that lands after the delete is not knowledge.
 */
async function savePreview<M extends PreviewSnapshot>(
  key: string,
  writes: PreviewWrites,
  epoch: number,
  fingerprint: string,
  payload: M[],
  write: (messages: M[]) => Promise<boolean>,
): Promise<boolean> {
  if (writes.epoch !== epoch) return false;
  let stored: boolean;
  try {
    stored = await write(payload);
  } catch {
    return false;
  }
  if (stored && writes.epoch === epoch) savedPreviews.set(key, fingerprint);
  return stored;
}
