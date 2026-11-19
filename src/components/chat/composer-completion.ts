// Pure trigger logic of the composer autocompletes (S3 and S4 of
// `odd/tasks/composer-inputs.md`). No Vue, no contracts, no DOM: every rule the
// two autocompletes obey is here, so the wiring in `Composer.vue` only asks
// "which trigger is open at this caret?" and "what does completing this insert?".
//
// The trigger rule, stated honestly and simply: the caret sits inside the word
// that starts just after the last whitespace, and that word must START with the
// trigger character.
//
//   `/fix`            → a command (only when the draft starts with it: a slash
//                       mid-sentence is prose, not an invocation)
//   `look at @src/ap` → a path mention
//   `foo@bar`         → nothing: the word starts with `f`, not with `@`, so an
//                       address or an identifier never opens a picker
//   `@src/app.ts `    → nothing: the trailing space ended the word
//
// A trigger only decides whether a list is offered. What a pick does is decided
// by the caller (a command completes to `/name ` and is sent through the existing
// prompt path; a path completes to `@relative/path`).

/** Which autocomplete a caret position asks for. */
export type CompletionKind = "command" | "path";

/** One open autocomplete: what kind, where it starts, and what is typed so far. */
export interface CompletionTrigger {
  kind: CompletionKind;
  /** Index in the draft of the trigger character (`/` or `@`). */
  start: number;
  /** The text typed after the trigger character, up to the caret. */
  query: string;
}

/** One row of the autocomplete: what it shows and what picking it inserts. */
export interface SuggestionItem {
  /** Stable key: the command name without its slash, or the workspace-relative path. */
  key: string;
  /** What the row shows, trigger character included. */
  label: string;
  /** Secondary text (a command's description), absent when there is none. */
  hint?: string;
  /** Short right-aligned mark: a command's source, or the kind of a path. */
  badge?: string;
  /** The text that replaces the trigger token. */
  insert: string;
  /** A directory keeps the picker open on the next segment. */
  directory?: boolean;
}

/** A path query split into the directory to list and the segment being typed. */
export interface PathQuery {
  /** Workspace-relative directory (`""` is the workspace root). */
  dir: string;
  /** The last segment, which filters that directory's entries. */
  filter: string;
}

/**
 * The trigger open at `caret`, or `null` when nothing should be offered.
 *
 * `caret` is the textarea's `selectionStart`: a list is offered where the human
 * is typing, never where the pointer happens to be.
 */
export function findCompletionTrigger(
  text: string,
  caret: number,
): CompletionTrigger | null {
  const at = Math.max(0, Math.min(caret, text.length));
  // The word the caret sits in, back to its last separator.
  let start = at;
  while (start > 0 && !isSeparator(text[start - 1] ?? "")) start -= 1;
  const word = text.slice(start, at);
  if (word.startsWith("/")) {
    // S3: only the first word of the draft is a command. `/` anywhere else is
    // prose (a date, a path, "y/o"), and offering commands there would turn
    // ordinary writing into an invocation.
    return start === 0 ? { kind: "command", start, query: word.slice(1) } : null;
  }
  if (word.startsWith("@")) {
    return { kind: "path", start, query: word.slice(1) };
  }
  return null;
}

/** Whitespace separates words: a mention or a command never contains one. */
function isSeparator(character: string): boolean {
  return character === " " || character === "\n" || character === "\t" || character === "\r";
}

/**
 * Splits a path query at its last slash: `src/comp` lists `src` filtering by
 * `comp`, and `src/` lists `src` with no filter. One level at a time, because the
 * walk is lazy: the picker never reads a tree the human is not looking at.
 */
export function splitPathQuery(query: string): PathQuery {
  const slash = query.lastIndexOf("/");
  if (slash === -1) return { dir: "", filter: query };
  return { dir: query.slice(0, slash), filter: query.slice(slash + 1) };
}

/**
 * The draft after picking one suggestion: the trigger token is replaced and the
 * rest of the text is kept, so completing mid-sentence does not eat what follows.
 *
 * A directory ends in `/` (the next segment is what the human wants next) and
 * every other pick ends in a space, which ends the word and closes the picker.
 */
export function applySuggestion(
  text: string,
  trigger: CompletionTrigger,
  caret: number,
  insert: string,
): { text: string; caret: number } {
  const at = Math.max(0, Math.min(caret, text.length));
  const head = text.slice(0, trigger.start);
  const tail = text.slice(at);
  return {
    text: `${head}${insert}${tail}`,
    caret: trigger.start + insert.length,
  };
}
