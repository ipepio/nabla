// Shared provider/model catalogue: the chat's model picker and the settings pane
// read the same answer, from one place.
//
// Both surfaces read what Rust reports — `list_provider_models` groups pi's
// `models.json` and pi's own cached catalogue by provider — and both of them
// *change* it: the settings pane hides a model, saves or removes one, discovers
// models, saves or deletes a provider, signs an account in. When each surface
// loaded the list on its own, the chat's picker kept the answer it took on mount
// (plus a window-focus refresh) and the settings pane had no way to tell it that
// something moved: hiding a model had no visible effect until the app was
// reopened.
//
// So the state lives at module scope, like `src/lib/session-store.ts`: one
// instance per app, every importer reading and writing the same refs, so a write
// in one surface is visible in the other without either of them knowing the
// other exists.
//
// Three rules make that cheap and safe:
//
// - Single flight: concurrent callers share the read that is already on its way,
//   so opening the pane while the chat refreshes costs one call, not two.
// - Debounce: a burst of requests — several switches flipped, one discovery that
//   adds N models, a write that also refreshes its pane — collapses into ONE
//   read, so the picker is not reloaded once per write.
// - Keep the last good answer: a failed read reports `providerModelsError` and
//   leaves `providerModels` exactly as it was. An unreadable file is not proof
//   that a provider has no models, and blanking the picker would be a lie.
//
// There is no polling. The list is read on mount, on window focus (the moment a
// browser sign-in becomes visible again) and after a write, and nowhere else.
import { ref } from "vue";
import { commandErrorMessage, listProviderModels } from "../contracts/commands";
import type { ProviderModels } from "../contracts/types";

/** Every provider with the models pi would use, as last read. */
export const providerModels = ref<ProviderModels[]>([]);

/** A read is on its way; a surface may paint this instead of guessing. */
export const providerModelsLoading = ref(false);

/**
 * Why the last read failed, `null` after a good one. The data are untouched when
 * this is set, so the surfaces keep rendering the previous answer.
 */
export const providerModelsError = ref<string | null>(null);

/** One read has answered since the app started (the first-fast-path guard). */
let loaded = false;

/** The answer on screen may predate a write; the next read must not be skipped. */
let stale = true;

/** The read in flight, if any: concurrent callers join it (single flight). */
let inFlight: Promise<ProviderModels[]> | null = null;

/** The debounced read of a burst of requests, `null` when no burst is pending. */
let burst: Promise<ProviderModels[]> | null = null;
let burstSettler: {
  resolve: (list: ProviderModels[]) => void;
  reject: (cause: unknown) => void;
} | null = null;

/**
 * A few milliseconds: long enough that the requests of one user action arrive as
 * one read (a discovery that adds N models, a write whose pane also refreshes),
 * short enough that a single switch feels immediate.
 */
const BURST_MS = 16;

export interface LoadProviderModelsOptions {
  /**
   * Read again even when a read already answered. A mount wants the default,
   * which joins the answer on screen; a write — or the window-focus refresh —
   * asks for `force`, because its whole point is an answer newer than the one we
   * hold.
   */
  force?: boolean;
}

/**
 * Reads the catalogue, or joins the read that is already happening.
 *
 * Unforced callers with a fresh answer get it back without a call. A forced
 * caller that arrives while a read is in flight is not answered with that read
 * (it started before the write) but with the burst that runs behind it, so a
 * write is never invisible. It never rejects: a failure is reported through
 * `providerModelsError` and resolves with the list still on screen.
 */
export function loadProviderModels(
  options: LoadProviderModelsOptions = {},
): Promise<ProviderModels[]> {
  const force = options.force === true;
  if (inFlight !== null) {
    // Single flight: an unforced caller shares the running read, a forced one
    // waits for it and then reads again.
    return force ? scheduleBurst() : inFlight;
  }
  if (!force && loaded && !stale) return Promise.resolve(providerModels.value);
  return scheduleBurst();
}

/**
 * Marks the catalogue as out of date without reading it: the next mount, window
 * focus or pane being opened reloads it instead of painting the stale list.
 *
 * This is the cheap signal for "something may have moved but nothing on screen
 * is waiting for the answer" — a pane reopening is the usual caller. A surface
 * that must see the change now asks `loadProviderModels({ force: true })`
 * instead, which is what a completed write does.
 */
export function invalidateProviderModels(): void {
  stale = true;
}

/** Coalesces every request of one burst into the same promise. */
function scheduleBurst(): Promise<ProviderModels[]> {
  if (burst === null) {
    burst = new Promise<ProviderModels[]>((resolve, reject) => {
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
    running === null ? readCatalogue() : running.then(readCatalogue, readCatalogue);
  read.then(settler.resolve, settler.reject);
}

/**
 * The one call to `list_provider_models`. It never rejects: the previous list
 * survives a failure and the reason travels in `providerModelsError`, so a picker
 * caught mid-write keeps offering what it already showed.
 */
function readCatalogue(): Promise<ProviderModels[]> {
  providerModelsLoading.value = true;
  const read = listProviderModels()
    .then((list) => {
      providerModels.value = list;
      providerModelsError.value = null;
      loaded = true;
      stale = false;
      return list;
    })
    .catch((cause: unknown) => {
      providerModelsError.value = commandErrorMessage(cause);
      // Still stale: the next action retries instead of trusting an old answer.
      stale = true;
      return providerModels.value;
    })
    .finally(() => {
      inFlight = null;
      providerModelsLoading.value = false;
    });
  inFlight = read;
  return read;
}
