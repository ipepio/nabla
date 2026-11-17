// Shared MCP server list: the settings pane's MCP section and everything that
// writes pi's `mcp.json` read one answer from one place.
//
// The read is expensive — `pi mcp list --json` boots the whole CLI and connects
// every enabled server (~0.7 s measured) — so awaiting it on mount is a lie the
// human pays for: the pane sat on "No MCP servers configured yet" for most of a
// second, which is exactly the wrong sentence on a machine that has servers.
//
// So the state lives at module scope, like `src/lib/provider-models.ts` and
// `src/lib/session-store.ts`: one instance per app run, every importer reading
// and writing the same refs. A pane mounted after the first answer paints the
// previous list in the same frame and lets the read run behind it.
//
// Three rules make that cheap and safe:
//
// - Single flight: concurrent callers share the read that is already on its way,
//   so rebuilding the pane while a read runs costs one call, not two.
// - Debounce: a burst of requests — a mount plus the focus refresh, a write that
//   also asks for a newer answer — collapses into ONE read.
// - Keep the last good answer: a failed read reports `error` and leaves `servers`
//   exactly as it was. An unreadable `mcp.json`, or a `pi mcp list` that timed
//   out, is not proof that there are no servers, and blanking the list would be a
//   lie the human cannot tell apart from "nothing configured".
//
// `settled` is what lets a surface distinguish "nobody has answered yet" from
// "pi answered and the list is empty": only the second one is an empty state.
//
// There is no polling. The list is read on mount (in the background), on demand
// (`loadMcpServers({ force: true })`, the Refresh action) and wherever a write
// lands — and a write does not read at all: `mcp_add`, `mcp_remove` and
// `mcp_logout` already answer with the listing they took after the write, so
// `applyMcpServers` adopts that answer instead of booting the CLI a second time.
import { ref } from "vue";
import { commandErrorMessage, mcpList } from "../contracts/commands";
import type { McpServerSummary } from "../contracts/types";

/** Every server pi's MCP configuration holds, as last read. */
export const servers = ref<McpServerSummary[]>([]);

/** A read is on its way; a surface may paint this instead of guessing. */
export const loading = ref(false);

/**
 * Why the last read failed, `null` after a good one. The data are untouched when
 * this is set, so the surfaces keep rendering the previous answer.
 */
export const error = ref<string | null>(null);

/**
 * A read (or an adopted write) has answered since this app started, so the list
 * on screen is pi's answer and not a default. Until this is `true`, an empty
 * `servers` means "still reading", never "nothing configured".
 */
export const settled = ref(false);

/** One read has answered (or one write adopted a listing) this session. */
let loaded = false;

/** The answer on screen may predate a write; the next read must not be skipped. */
let stale = true;

/**
 * Bumped by `applyMcpServers`. A read that was already on its way when a write
 * landed describes the file *before* that write, so its answer is discarded
 * instead of undoing the row the human just removed.
 */
let adoptStamp = 0;

/** The read in flight, if any: concurrent callers join it (single flight). */
let inFlight: Promise<McpServerSummary[]> | null = null;

/** The debounced read of a burst of requests, `null` when no burst is pending. */
let burst: Promise<McpServerSummary[]> | null = null;
let burstSettler: {
  resolve: (list: McpServerSummary[]) => void;
  reject: (cause: unknown) => void;
} | null = null;

/**
 * A few milliseconds: long enough that the requests of one user action arrive as
 * one read, short enough that a single refresh feels immediate.
 */
const BURST_MS = 16;

export interface LoadMcpServersOptions {
  /**
   * Read again even when a read already answered. A mount wants the default,
   * which joins the answer on screen; the Refresh action — or anything whose
   * whole point is an answer newer than the one we hold — asks for `force`.
   */
  force?: boolean;
}

/**
 * Reads the list, or joins the read that is already happening.
 *
 * Unforced callers with a fresh answer get it back without a call. A forced
 * caller that arrives while a read is in flight is not answered with that read
 * (it started before the request) but with the burst that runs behind it, so a
 * refresh is never silently skipped. It never rejects: a failure is reported
 * through `error` and resolves with the list still on screen.
 */
export function loadMcpServers(
  options: LoadMcpServersOptions = {},
): Promise<McpServerSummary[]> {
  const force = options.force === true;
  if (inFlight !== null) {
    // Single flight: an unforced caller shares the running read, a forced one
    // waits for it and then reads again.
    return force ? scheduleBurst() : inFlight;
  }
  if (!force && loaded && !stale) return Promise.resolve(servers.value);
  return scheduleBurst();
}

/**
 * Marks the list as out of date without reading it: the next mount or refresh
 * reloads it instead of painting the stale rows as if nothing had moved.
 *
 * This is the cheap signal for "something may have moved but nothing on screen is
 * waiting for the answer" — a pane closing is the usual caller, because pi's
 * `mcp.json` can be edited outside the app. A surface that must see the change
 * now asks `loadMcpServers({ force: true })` instead.
 */
export function invalidateMcpServers(): void {
  stale = true;
}

/**
 * Adopts a listing a write already answered with (`mcp_add`, `mcp_remove`,
 * `mcp_logout` each return the listing taken after the write). The store stays
 * the single writer of `servers`, so the pane never patches its own copy, and
 * nothing boots `pi mcp list` a second time to hear the same answer.
 *
 * A read that was already on its way is invalidated: its answer describes the
 * file before the write, so it must not overwrite what the write observed.
 */
export function applyMcpServers(list: McpServerSummary[]): void {
  adoptStamp += 1;
  servers.value = list;
  error.value = null;
  loading.value = false;
  settled.value = true;
  loaded = true;
  stale = false;
}

/** Coalesces every request of one burst into the same promise. */
function scheduleBurst(): Promise<McpServerSummary[]> {
  if (burst === null) {
    burst = new Promise<McpServerSummary[]>((resolve, reject) => {
      burstSettler = { resolve, reject };
    });
    setTimeout(flushBurst, BURST_MS);
  }
  return burst;
}

/** The debounce expired: one read for the whole burst. */
function flushBurst(): void {
  const settler = burstSettler;
  const running = inFlight;
  burst = null;
  burstSettler = null;
  if (settler === null) return;
  // A read that was already on its way when the burst was queued is older than
  // the requests this burst is about, so the burst reads after it settles.
  const read =
    running === null
      ? readServers()
      : running.then(readServers, readServers);
  read.then(settler.resolve, settler.reject);
}

/**
 * The one call to `mcp_list`. It never rejects: the previous list survives a
 * failure and the reason travels in `error`, so a pane caught mid-write keeps
 * offering the rows it already showed.
 */
function readServers(): Promise<McpServerSummary[]> {
  const issuedAt = adoptStamp;
  loading.value = true;
  const read = mcpList()
    .then((list) => {
      // A write landed while this read ran: its listing is newer than this one,
      // so the rows on screen stay exactly as the write left them.
      if (adoptStamp !== issuedAt) return servers.value;
      servers.value = list;
      error.value = null;
      settled.value = true;
      loaded = true;
      stale = false;
      return list;
    })
    .catch((cause: unknown) => {
      if (adoptStamp !== issuedAt) return servers.value;
      error.value = commandErrorMessage(cause);
      // Still stale: the next action retries instead of trusting an old answer.
      stale = true;
      return servers.value;
    })
    .finally(() => {
      inFlight = null;
      loading.value = false;
    });
  inFlight = read;
  return read;
}
