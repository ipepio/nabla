// The one thing that moves a transcript's scroll offset: the "follow the
// newest line while the reader is already there" behaviour both the conversation
// and the read-only subagent view need.
//
// It exists because that behaviour arrives from many places at once — a streamed
// delta, a committed turn, a tool chip, a question, a window resize, the
// transcript's own scroll event — and each of them used to call a scroll of its
// own. Several of those land in the same frame, so all but the last were wasted
// work on a moving target. `request()` therefore arms at most one frame, and the
// scroll that lands is the one that reads the height of the finished layout.
//
// The pin is the reader's, not the panel's: it drops the moment the human moves
// up the conversation (`handleScroll`) and comes back when they return to the
// end, and a pending frame never drags a reader who left in the meantime.
//
// No Vue and no DOM type beyond the three numbers a scroller has, so the
// behaviour is testable without a browser (`tests/transcript-follow.test.ts`) and
// the views keep ownership of their refs, listeners and observers.

/** A scrolling element, by the only three numbers this needs. */
export interface TranscriptScroller {
  scrollTop: number;
  readonly scrollHeight: number;
  readonly clientHeight: number;
}

/** Distance to the end under which the view counts as glued to the newest line. */
export const BOTTOM_SLACK_PX = 48;

export interface TranscriptFollowOptions {
  /** Arms the coalesced scroll for the next paint; `requestAnimationFrame`. */
  requestFrame?: (callback: () => void) => number;
  /** Drops one of those; `cancelAnimationFrame`. */
  cancelFrame?: (handle: number) => void;
  /** Overrides `BOTTOM_SLACK_PX`. */
  slackPx?: number;
}

/** Where the reader was, for growth that is added *above* them. */
export interface ReaderAnchor {
  scrollTop: number;
  scrollHeight: number;
}

export interface TranscriptFollow {
  /** The view follows growth. */
  pinned(): boolean;
  /** A frame is waiting to land. */
  pending(): boolean;
  /** Binds the scroller to follow; opening at its end. `null` detaches. */
  attach(scroller: TranscriptScroller | null): void;
  /** Forgets the scroller and cancels the frame it had armed. */
  detach(): void;
  /** Coalesced scroll to the end, only while the pin holds. */
  request(): void;
  /** Scrolls to the end now and pins the view to it. */
  jump(): void;
  /** Re-reads the pin from the live position, moving nothing. */
  syncPin(): void;
  /** The scroller's `scroll` event. */
  handleScroll(): void;
  /** The scroller or its content changed size. */
  handleResize(): void;
  /** Where the reader is now; `null` when there is nothing to follow. */
  snapshot(): ReaderAnchor | null;
  /** Puts the reader back after content above them was added. */
  restore(anchor: ReaderAnchor | null): void;
}

export function createTranscriptFollow(
  options: TranscriptFollowOptions = {},
): TranscriptFollow {
  const requestFrame =
    options.requestFrame ?? ((callback: () => void) => window.requestAnimationFrame(callback));
  const cancelFrame =
    options.cancelFrame ?? ((handle: number) => window.cancelAnimationFrame(handle));
  const slack = options.slackPx ?? BOTTOM_SLACK_PX;

  /** What is being followed, or `null` before mount and after unmount. */
  let scroller: TranscriptScroller | null = null;
  /** The reader is at (or near) the end, so growth may move the view. */
  let glued = true;
  /** Handle of the armed frame, `null` when none is waiting. */
  let frame: number | null = null;
  /** The offset this controller itself last scrolled to (see `handleScroll`). */
  let programmaticOffset = 0;

  function atEnd(element: TranscriptScroller): boolean {
    return element.scrollHeight - element.scrollTop - element.clientHeight <= slack;
  }

  /**
   * The scroll itself. Assigning past the maximum is clamped by the browser, so
   * a conversation shorter than its viewport needs no special case.
   */
  function scrollToEnd(): void {
    const element = scroller;
    if (element === null) return;
    element.scrollTop = element.scrollHeight;
    programmaticOffset = element.scrollTop;
    glued = true;
  }

  function cancel(): void {
    if (frame === null) return;
    cancelFrame(frame);
    frame = null;
  }

  function detach(): void {
    cancel();
    scroller = null;
    programmaticOffset = 0;
  }

  function request(): void {
    if (scroller === null || frame !== null) return;
    frame = requestFrame(() => {
      frame = null;
      // The frame reads the pin and the height when it runs, not when it was
      // armed: a reader who left in between is left alone.
      if (scroller === null || !glued) return;
      scrollToEnd();
    });
  }

  return {
    pinned: () => glued,
    pending: () => frame !== null,

    attach(next: TranscriptScroller | null): void {
      if (next === null) {
        detach();
        return;
      }
      scroller = next;
      programmaticOffset = 0;
      // A freshly attached transcript has no position to preserve: open at the end.
      scrollToEnd();
    },

    detach,

    request,

    jump(): void {
      cancel();
      glued = true;
      scrollToEnd();
    },

    syncPin(): void {
      const element = scroller;
      if (element === null) return;
      glued = atEnd(element);
    },

    /**
     * A scroll event does not say who moved the view, and content can grow
     * between this controller's own assignment and the event it produces. An
     * offset at or below the one it set is therefore read as "still at the end,
     * follow the new bottom"; only an offset above it is the human moving up.
     */
    handleScroll(): void {
      const element = scroller;
      if (element === null) return;
      if (glued && element.scrollTop >= programmaticOffset) {
        request();
        return;
      }
      glued = atEnd(element);
    },

    /**
     * A resize reflows the transcript. A glued view is put back on the end in
     * the same frame it was reflowed in — one frame of lag would be visible
     * during a window resize — while a free view only re-reads its pin.
     */
    handleResize(): void {
      const element = scroller;
      if (element === null) return;
      if (glued) {
        scrollToEnd();
        return;
      }
      glued = atEnd(element);
    },

    snapshot(): ReaderAnchor | null {
      const element = scroller;
      if (element === null) return null;
      return { scrollTop: element.scrollTop, scrollHeight: element.scrollHeight };
    },

    /**
     * Content added *above* the reader grows `scrollHeight` without touching
     * `scrollTop`, which moves everything they were reading down by the same
     * amount. Re-applying the difference keeps the line they were on the line
     * they are on. Used when older turns are revealed inside a view that is not
     * glued to the end.
     */
    restore(anchor: ReaderAnchor | null): void {
      const element = scroller;
      if (element === null || anchor === null) return;
      const grown = element.scrollHeight - anchor.scrollHeight;
      if (grown === 0) return;
      element.scrollTop = Math.max(0, anchor.scrollTop + grown);
    },
  };
}
