// P2 of `chat-polish.md`: what the human typed and did not send survives leaving
// the session (or the app) and coming back. The composer is the only storage
// surface, so what it holds is the draft of what it is addressing.
//
// `localStorage` is first used here. Nothing about it may surface: a store that
// is unavailable, disabled (private windows) or full degrades to "this feature
// simply does not persist", and reading `window.localStorage` itself can throw,
// so every access is wrapped.
import { onScopeDispose, ref, watch, type Ref } from "vue";

/** Every draft key starts here; a session id or `new.<workspaceId>` follows. */
const DRAFT_KEY_PREFIX = "nabla.draft.";

/** Longest a keystroke waits before it is written. */
const DRAFT_SAVE_MS = 300;

/**
 * The draft key of the conversation on screen.
 *
 * A session has its own key (`nabla.draft.<sessionId>`); the state before the
 * first send has no session yet — `ensureSession` creates it when the prompt is
 * handed to pi — so that state is keyed by workspace instead
 * (`nabla.draft.new.<workspaceId>`), which is what keeps a typed prompt when the
 * human moves away and comes back without sending it.
 */
export function draftKeyFor(
  sessionId: string | null,
  workspaceId: number | null,
): string | null {
  if (sessionId !== null) return `${DRAFT_KEY_PREFIX}${sessionId}`;
  if (workspaceId !== null) return `${DRAFT_KEY_PREFIX}new.${workspaceId}`;
  return null;
}

function readDraft(key: string | null): string {
  if (key === null) return "";
  try {
    return window.localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
}

function writeDraft(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Unavailable or full: the draft is simply not persisted.
  }
}

function forgetDraft(key: string | null): void {
  if (key === null) return;
  try {
    window.localStorage.removeItem(key);
  } catch {
    // Same as above: nothing to report, there is no surface for it.
  }
}

export interface PromptDraft {
  /** The composer's text (`v-model`). */
  text: Ref<string>;
  /**
   * Empties the composer and forgets the draft, because it is no longer unsent
   * (sent or queued). Extra keys are forgotten too: sending the first prompt of a
   * workspace creates its session, so the text that was typed under
   * `nabla.draft.new.<workspaceId>` must go with the one under the new session id.
   */
  clear: (...alsoForget: Array<string | null>) => void;
}

/**
 * Binds one composer ref to the draft of `key()`, which the caller recomputes
 * whenever the conversation on screen changes.
 *
 * The key is read at every use instead of being captured once: the panel's key
 * depends on the active session, which the shared store changes under it.
 */
export function usePromptDraft(
  key: () => string | null,
  saveMs: number = DRAFT_SAVE_MS,
): PromptDraft {
  const text = ref("");
  /** Where the armed write lands, and what it writes: both captured when armed,
   * so a switch settles the text under the key it was typed for. */
  let armedKey: string | null = null;
  let armedText = "";
  let timer: number | null = null;

  function cancel(): void {
    if (timer === null) return;
    window.clearTimeout(timer);
    timer = null;
  }

  function writeArmed(): void {
    const target = armedKey;
    const value = armedText;
    armedKey = null;
    armedText = "";
    cancel();
    if (target === null || value === "") return;
    writeDraft(target, value);
  }

  function arm(target: string, value: string): void {
    armedKey = target;
    armedText = value;
    cancel();
    timer = window.setTimeout(() => {
      timer = null;
      writeArmed();
    }, saveMs);
  }

  watch(text, (next) => {
    const target = key();
    if (target === null) {
      // Nothing addresses this text, so there is nowhere to persist it.
      armedKey = null;
      armedText = "";
      cancel();
      return;
    }
    if (next === "") {
      // An emptied composer is an empty draft: keeping the stored text would
      // resurrect words the human deleted.
      armedKey = null;
      armedText = "";
      cancel();
      forgetDraft(target);
      return;
    }
    arm(target, next);
  });

  watch(
    key,
    (target) => {
      // What was typed before the switch belongs to the key it was typed under.
      // Settling the armed write first is what makes switching never lose it and
      // never land it on the session the human just opened.
      writeArmed();
      // A draft never overwrites a composer the human is already typing in: only
      // an empty one takes the stored text.
      if (text.value !== "") return;
      const stored = readDraft(target);
      if (stored === "") return;
      text.value = stored;
    },
    { immediate: true },
  );

  function clear(...alsoForget: Array<string | null>): void {
    armedKey = null;
    armedText = "";
    cancel();
    const current = key();
    text.value = "";
    forgetDraft(current);
    for (const other of alsoForget) {
      if (other !== current) forgetDraft(other);
    }
  }

  // Leaving the panel is not a reason to lose the last keystrokes.
  onScopeDispose(writeArmed);

  return { text, clear };
}
