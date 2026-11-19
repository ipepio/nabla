// P1 of `chat-polish.md`: the live assistant tail arrives one delta at a time,
// and every delta re-parses the whole message in `AssistantText` (fenced code
// segments, inline markup) — thousands of parses for one long answer. This
// throttles the *live* tail only.
//
// The committed turns never go through here: they render `message.text` from the
// session buffer, so the text that stays on screen is byte-identical to what pi
// sent, independent of any timing in this module.
import { onScopeDispose, ref, watch, type Ref } from "vue";

/** Longest the live tail may lag behind pi: roughly eight repaints per second. */
export const STREAM_THROTTLE_MS = 120;

/**
 * A throttled mirror of a string that grows while a turn streams.
 *
 * `identity` is the handle of the turn the text belongs to (the store's live
 * message id). It exists because text alone cannot tell "the same turn grew"
 * from "another turn replaced it", and the second case must never wait for a
 * timer: it would paint the previous turn's words under the new one.
 *
 * @param source the text as the store has it, updated per delta.
 * @param identity the id of the turn `source` currently describes.
 */
export function useThrottledText(
  source: Ref<string>,
  identity: () => string | null,
  intervalMs: number = STREAM_THROTTLE_MS,
): Ref<string> {
  /** What is painted right now: the throttled tail. */
  const text = ref(source.value);
  /** Deadline of the armed trailing publish, `null` when none is pending. */
  let timer: number | null = null;
  /** When the last publish happened, so the leading edge is honoured too. */
  let publishedAt = 0;
  /** The turn the painted text belongs to. */
  let paintedIdentity = identity();

  function cancel(): void {
    if (timer === null) return;
    window.clearTimeout(timer);
    timer = null;
  }

  function publish(next: string): void {
    cancel();
    paintedIdentity = identity();
    text.value = next;
    publishedAt = Date.now();
  }

  /**
   * The one place that decides whether the new tail is published now or after
   * the interval.
   *
   * A publish is forced when waiting would paint something *wrong* rather than
   * merely old, which is exactly the turn transition the commit depends on:
   *
   * - the turn ended. `flushStreaming` in the store moves `streamingText` into a
   *   committed message and clears the live field. The committed message paints
   *   in the same render, so a throttled value still holding the old tail would
   *   render the answer twice (message + live bubble) until the timer fired, or
   *   paint a tail that no longer belongs to any live turn. Publishing the new
   *   (empty) value in that same tick makes the live bubble disappear with the
   *   message, which is the guarantee: **nothing survives the commit, and
   *   nothing of the tail is lost.**
   * - another turn started. The painted text describes a different message id,
   *   so it is not the prefix of anything the new turn will say.
   * - the tail no longer extends what is painted. Growth is the only thing a
   *   streaming turn does, so anything else is a replacement (a cleared tail, a
   *   switch to another session's buffer) and cannot be throttled.
   */
  function apply(): void {
    const next = source.value;
    if (identity() !== paintedIdentity || !next.startsWith(text.value)) {
      publish(next);
      return;
    }
    const wait = intervalMs - (Date.now() - publishedAt);
    if (wait <= 0) {
      publish(next);
      return;
    }
    // An armed deadline is already the earliest one for this painted value.
    if (timer !== null) return;
    timer = window.setTimeout(() => {
      timer = null;
      publish(source.value);
    }, wait);
  }

  watch([source, identity], apply);
  // A pending publish outliving the panel would write into a dead ref.
  onScopeDispose(cancel);

  return text;
}
