// How many turn groups of a transcript are mounted at once.
//
// A conversation that has run for a while carries hundreds of turns, and every
// turn mounts its own disclosure, chips and formatted answer. Mounting all of
// them to show the newest one is the cost this window removes: the newest page is
// mounted, older pages are revealed on request, in pages, and nothing the human
// already revealed is taken away from them afterwards.
//
// Two ongoing conversations must not share a window, so the window resets when the
// thing it is showing changes (another session, another child). A transcript that
// arrives *after* mount — the usual case, since history is read once the panel is
// already on screen — is bounded the same way, because the window is derived from
// the list rather than snapshotted from it.
//
// Revealing is the view's business (a control, an anchored scroll): this module
// only owns *what* is mounted.
import { computed, ref, watch, type ComputedRef, type Ref } from "vue";

/** Turns mounted on the first paint of a conversation. */
export const INITIAL_VISIBLE_TURNS = 12;
/** Turns one "show earlier" reveals. */
export const OLDER_TURNS_PER_REVEAL = 12;

export interface TurnWindowOptions {
  /** Overrides `INITIAL_VISIBLE_TURNS`. */
  initial?: number;
  /** Overrides `OLDER_TURNS_PER_REVEAL`. */
  page?: number;
}

export interface TurnWindow<T> {
  /** The turns to mount, oldest first: a contiguous run ending at the newest. */
  groups: ComputedRef<T[]>;
  /** How many older turns exist above the window. */
  hiddenCount: ComputedRef<number>;
  /** There is something older to reveal. */
  hasOlder: ComputedRef<boolean>;
  /** Reveals one page of older turns; inert when nothing older is left. */
  showOlder(): void;
}

/**
 * Bounds a turn-group list to the newest page plus whatever was revealed.
 *
 * @param groups every turn of the conversation on screen, oldest first.
 * @param identity what the list belongs to (session id, open child); a change
 *   starts a fresh window, so one conversation's reveals never apply to another.
 */
export function useTurnWindow<T>(
  groups: Ref<readonly T[]>,
  identity: () => string | number | null,
  options: TurnWindowOptions = {},
): TurnWindow<T> {
  const initial = Math.max(1, options.initial ?? INITIAL_VISIBLE_TURNS);
  const page = Math.max(1, options.page ?? OLDER_TURNS_PER_REVEAL);

  /**
   * The oldest group the human asked to keep, as an index into the list, or
   * `null` while the window is still the automatic newest page. It is an index
   * and not a count because arrivals at the end must not shift it: a revealed
   * page stays mounted.
   */
  const floor = ref<number | null>(null);

  /** Where the automatic window starts: the newest `initial` turns. */
  const automaticStart = computed(() => Math.max(0, groups.value.length - initial));

  const start = computed(() => {
    const anchor = floor.value;
    if (anchor === null) return automaticStart.value;
    // `min` is the clamp that matters: a transcript that shrank below the
    // revealed floor (another read replaced it) must not ask for turns that are
    // gone, and a longer list simply keeps the revealed page mounted.
    return Math.min(anchor, automaticStart.value);
  });

  const hiddenCount = computed(() => start.value);
  const visible = computed(() => groups.value.slice(start.value));
  const hasOlder = computed(() => hiddenCount.value > 0);

  watch(identity, () => {
    floor.value = null;
  });

  return {
    groups: visible,
    hiddenCount,
    hasOlder,
    showOlder(): void {
      if (!hasOlder.value) return;
      floor.value = Math.max(0, start.value - page);
    },
  };
}
