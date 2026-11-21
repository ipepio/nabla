// Splitting assistant prose into blocks and fenced code, without a markdown
// dependency.
//
// pi streams plain text with constructs the transcript honours: fenced code
// (``` optionally followed by a language), ATX headings (`#` through `######`),
// runs of list rows (`- `, `* `, `+ ` and `N. `), inline `code` spans, emphasis
// and line breaks. Everything else is prose and stays exactly as it arrived —
// the view renders every piece through interpolations (or through `h()` with
// string children, which escapes the same way), so user content can never become
// markup.
//
// Blockquotes, tables and horizontal rules are deliberately absent: they are not
// part of what the transcript renders, so those characters stay literal prose.
//
// Fences are recognised first and sliced verbatim, so every construct here can
// only ever apply to prose: a `### heading` or a `- row` inside a fence stays
// exactly the text the model wrote.
//
// A fence that never closes is not an error: a stream mid-token has an open
// fence all the time, so it renders as a code block holding what exists so far.

/** A run of prose, with its inline code and emphasis already marked. */
export type InlinePart =
  | { type: "plain"; text: string }
  | { type: "inlineCode"; code: string }
  | { type: "bold"; text: string }
  | { type: "italic"; text: string }
  | { type: "link"; href: string; text: string };

/**
 * One piece of a message: prose with line breaks, a heading, a list, or a fenced
 * code block. The order of these blocks is the order they were written.
 */
export type TextSegment =
  | { type: "text"; parts: InlinePart[] }
  | {
      type: "heading";
      /** `1` for `#` through `6` for `######`; the hashes themselves are gone. */
      level: number;
      /** The heading's own text, inline markup and all. */
      parts: InlinePart[];
    }
  | {
      type: "list";
      /** `true` for a `N. ` run, `false` for a `-`/`*`/`+` one. */
      ordered: boolean;
      /** One entry per row, each one the row's text with its markup marked. */
      items: InlinePart[][];
    }
  | {
      type: "code";
      /** The fence's language tag, `""` when the fence carries none. */
      language: string;
      /** The block's exact source: no fences, no language line. */
      source: string;
      /** `false` while a stream has opened the fence and not closed it yet. */
      closed: boolean;
    };

/** A line that opens a fence: backticks first, info string after them. */
const FENCE_OPEN = /^[ \t]*```(.*)$/;
/** A line that closes one: the backticks and nothing else. */
const FENCE_CLOSE = /^[ \t]*```[ \t]*$/;
/**
 * An ATX heading: one to six `#`, one space, and text that is not blank. The
 * space is required, so `#hashtag` and a `###` line still waiting for its text
 * stay prose — a stream can be mid-token. Group 1 is the hashes, 2 the text.
 */
const HEADING = /^[ \t]*(#{1,6})[ \t]+(\S.*?)[ \t]*$/;
/**
 * One unordered list row: a `-`, `*` or `+` marker, one space, and text. The
 * marker needs the space, which is what leaves `*emphasis*` at the start of a
 * line as prose. Group 1 is the row's text.
 */
const BULLET_ITEM = /^[ \t]*[-*+][ \t]+(\S.*?)[ \t]*$/;
/**
 * One ordered list row: `N.`, one space, and text. The number is not kept — a
 * list renders as a list, and the marker is the view's business. Group 1 is the
 * row's text.
 */
const NUMBERED_ITEM = /^[ \t]*\d{1,9}\.[ \t]+(\S.*?)[ \t]*$/;
/**
 * Inline markdown, one combined pass. The alternation order matters: bold
 * (`**`/`__`) before italic (`*`/`_`), otherwise `**x**` would match the
 * single-asterisk rule first. Groups: 1 code, 2 bold-asterisk, 3 bold-underscore,
 * 4 italic-asterisk, 5 italic-underscore, 6 link text, 7 link href.
 */
const INLINE_MARKDOWN =
  /`([^`\n]+)`|\*\*([^*\n]+)\*\*|__([^_\n]+)__|\*([^*\n]+)\*|_([^_\n]+)_|\[([^\]\n]+)\]\(([^)\s]+)\)/g;

interface Line {
  text: string;
  /** Offset of the line's first character in the whole message. */
  start: number;
  /** Offset just past the line, including its newline when it has one. */
  next: number;
}

function linesOf(text: string): Line[] {
  const lines: Line[] = [];
  let start = 0;
  for (;;) {
    const newline = text.indexOf("\n", start);
    if (newline === -1) {
      lines.push({ text: text.slice(start), start, next: text.length });
      return lines;
    }
    lines.push({ text: text.slice(start, newline), start, next: newline + 1 });
    start = newline + 1;
  }
}

/** The language tag of a fence: its first word, lowercased nowhere, trimmed. */
function languageOf(info: string): string {
  const [first = ""] = info.trim().split(/\s+/);
  return first;
}

/** Only http(s) targets render as links; anything else stays inert text. */
function safeHref(href: string): string | null {
  return /^https?:\/\//i.test(href) ? href : null;
}

/** Turns one inline-markdown match into the part it describes. */
function partOf(match: RegExpExecArray): InlinePart {
  const [, code, boldStar, boldUnder, italicStar, italicUnder, linkText, href] =
    match;
  if (code !== undefined) return { type: "inlineCode", code };
  if (boldStar !== undefined || boldUnder !== undefined)
    return { type: "bold", text: boldStar ?? boldUnder };
  if (italicStar !== undefined || italicUnder !== undefined)
    return { type: "italic", text: italicStar ?? italicUnder };
  const safe = safeHref(href ?? "");
  if (linkText !== undefined && safe !== null)
    return { type: "link", href: safe, text: linkText };
  return { type: "plain", text: match[0] };
}

/**
 * One run of text, split around its inline code and emphasis. The unit is a
 * paragraph, a heading's text or a list row; each of them can carry markup.
 */
function inlinePartsOf(raw: string): InlinePart[] {
  const parts: InlinePart[] = [];
  let last = 0;
  for (const match of raw.matchAll(INLINE_MARKDOWN)) {
    const at = match.index ?? 0;
    if (at > last) parts.push({ type: "plain", text: raw.slice(last, at) });
    parts.push(partOf(match));
    last = at + match[0].length;
  }
  if (last < raw.length) parts.push({ type: "plain", text: raw.slice(last) });
  return parts;
}

/**
 * A run of plain prose (the lines between two blocks), or `null` when there is
 * nothing to paint. A newline that merely separated prose from a fence (or ended
 * the message) is structural and dropped by the caller, so an ordinary paragraph
 * does not gain a blank line next to its code block.
 */
function textSegment(raw: string): TextSegment | null {
  if (raw === "") return null;
  const parts = inlinePartsOf(raw);
  if (parts.length === 0) return null;
  return { type: "text", parts };
}

/**
 * One list row, or `null` for a line that is not one. The two markers are tried
 * in a fixed order so a row is never counted twice.
 */
function listRowOf(line: string): { ordered: boolean; text: string } | null {
  const bullet = BULLET_ITEM.exec(line);
  if (bullet !== null) return { ordered: false, text: bullet[1] ?? "" };
  const numbered = NUMBERED_ITEM.exec(line);
  if (numbered !== null) return { ordered: true, text: numbered[1] ?? "" };
  return null;
}

/**
 * A prose region — everything between two fences — split into the blocks it is
 * made of: paragraphs, ATX headings, and one list per run of consecutive rows.
 *
 * Only the lines that start a block are reinterpreted; everything else is kept
 * verbatim, newlines included, which is why a region without a heading or a list
 * row produces exactly the one `text` segment it always did. A run of two rows
 * is a list of two and a single stray row is a list of one: that is what the
 * model wrote, and silently demoting it would only hide it.
 */
function proseSegments(raw: string): TextSegment[] {
  const segments: TextSegment[] = [];
  /** The lines of the paragraph being accumulated; `[]` between paragraphs. */
  let paragraph: string[] = [];
  /** The run of list rows being accumulated, and its kind. */
  let list: { ordered: boolean; rows: string[] } | null = null;

  function flushParagraph(): void {
    if (paragraph.length === 0) return;
    const segment = textSegment(paragraph.join("\n"));
    if (segment !== null) segments.push(segment);
    paragraph = [];
  }

  function flushList(): void {
    if (list === null) return;
    segments.push({
      type: "list",
      ordered: list.ordered,
      items: list.rows.map(inlinePartsOf),
    });
    list = null;
  }

  for (const line of raw.split("\n")) {
    const heading = HEADING.exec(line);
    if (heading !== null) {
      flushParagraph();
      flushList();
      segments.push({
        type: "heading",
        level: (heading[1] ?? "#").length,
        parts: inlinePartsOf(heading[2] ?? ""),
      });
      continue;
    }
    const row = listRowOf(line);
    if (row !== null) {
      flushParagraph();
      // A row of the other kind closes the run: `1. a` then `- b` is two lists,
      // not one list that changed its mind.
      if (list === null || list.ordered !== row.ordered) {
        flushList();
        list = { ordered: row.ordered, rows: [] };
      }
      list.rows.push(row.text);
      continue;
    }
    flushList();
    paragraph.push(line);
  }
  flushParagraph();
  flushList();
  return segments;
}

/** Cuts the newline that ends a prose region sitting in front of a fence. */
function trimTrailingNewline(raw: string): string {
  return raw.endsWith("\n") ? raw.slice(0, -1) : raw;
}

/**
 * Segments one message: prose blocks (paragraphs, headings, lists) and fenced
 * code, in the order they were written.
 *
 * Fences are recognised only at the start of a line (`^` after any leading
 * spaces or tabs), which is what keeps a stray ``` inside a sentence as prose.
 * An unterminated fence takes the rest of the message as its source.
 */
export function segmentsOf(text: string): TextSegment[] {
  const segments: TextSegment[] = [];
  const lines = linesOf(text);
  /** Offset where the prose being accumulated starts. */
  let proseStart = 0;
  let index = 0;

  while (index < lines.length) {
    const line = lines[index];
    const open = FENCE_OPEN.exec(line.text);
    if (open === null) {
      index += 1;
      continue;
    }

    // Everything before the fence is prose, blocks and all.
    segments.push(
      ...proseSegments(trimTrailingNewline(text.slice(proseStart, line.start))),
    );

    const language = languageOf(open[1]);
    const sourceStart = line.next;
    let closeIndex = -1;
    for (let probe = index + 1; probe < lines.length; probe += 1) {
      if (FENCE_CLOSE.test(lines[probe].text)) {
        closeIndex = probe;
        break;
      }
    }

    if (closeIndex === -1) {
      // Still streaming: the block is what exists so far.
      segments.push({
        type: "code",
        language,
        source: text.slice(sourceStart),
        closed: false,
      });
      return segments;
    }

    segments.push({
      type: "code",
      language,
      source: trimTrailingNewline(
        text.slice(sourceStart, lines[closeIndex].start),
      ),
      closed: true,
    });
    proseStart = lines[closeIndex].next;
    index = closeIndex + 1;
  }

  segments.push(
    ...proseSegments(trimTrailingNewline(text.slice(proseStart))),
  );
  return segments;
}

/**
 * How far into `text` the blocks of an append-only stream are settled for good.
 *
 * A block is settled when the line that starts it is *complete* (it ends with a
 * newline, so the next append cannot touch it) and the parser would start a
 * fresh block there. That is the whole contract: `segmentsOf` parses a prose
 * region line by line and flushes a paragraph or a list run when it meets a
 * heading, a row of the other list kind, or prose after a non-prose line, so a
 * split at one of those lines produces exactly the blocks one full parse does —
 * and nothing else does, which is why an open paragraph or an open list run is
 * never split (see `tests/segments-cache.test.ts`).
 *
 * Two kinds of settled line exist:
 *
 * - a prose line that starts a block: an ATX heading, a list row that does not
 *   continue the run above it, and prose that follows a heading or a list row;
 * - the closing line of a fence, which settles everything up to and including
 *   itself; an opening fence settles the prose above it, so a code block that
 *   is still streaming no longer re-parses the answer that introduced it.
 *
 * The scan is incremental: it remembers where it stopped and the state it needs
 * (`at`, `inFence`, `last`), so a delta reads only the lines that arrived since
 * the previous call. A plain formatted answer therefore costs one line read per
 * delta instead of one full scan for nothing — which is what made the cache
 * slower than no cache for prose without fences.
 */
type ProseLine = "none" | "prose" | "heading" | "ordered" | "unordered";

interface Boundaries {
  /** Offset just past the last complete line already read. */
  at: number;
  /** A fence has opened and its closing line has not been read yet. */
  inFence: boolean;
  /** The kind of the last complete prose line read, `"none"` before the first. */
  last: ProseLine;
  /** Offset where the text settled; `0` before anything settles. */
  settled: number;
}

/** A scan that has not read anything yet. */
function freshBoundaries(): Boundaries {
  return { at: 0, inFence: false, last: "none", settled: 0 };
}

/** Moves the settled offset forward; it never goes back. */
function settle(boundaries: Boundaries, offset: number): void {
  if (offset > boundaries.settled) boundaries.settled = offset;
}

/** Reads the complete lines of `text` that `boundaries` has not seen yet. */
function scanBoundaries(boundaries: Boundaries, text: string): void {
  for (;;) {
    const newline = text.indexOf("\n", boundaries.at);
    // The last line can still grow, so it is never proof of anything.
    if (newline === -1) return;
    const start = boundaries.at;
    const line = text.slice(start, newline);
    boundaries.at = newline + 1;

    if (boundaries.inFence) {
      if (!FENCE_CLOSE.test(line)) continue;
      boundaries.inFence = false;
      boundaries.last = "none";
      settle(boundaries, newline + 1);
      continue;
    }
    if (FENCE_OPEN.test(line)) {
      boundaries.inFence = true;
      boundaries.last = "none";
      settle(boundaries, start);
      continue;
    }
    if (HEADING.test(line)) {
      boundaries.last = "heading";
      settle(boundaries, start);
      continue;
    }
    const row = listRowOf(line);
    if (row !== null) {
      const kind = row.ordered ? "ordered" : "unordered";
      // A row of the same kind continues the run above it: that run can still
      // gain a row, so its start is not a boundary yet.
      if (boundaries.last !== kind) settle(boundaries, start);
      boundaries.last = kind;
      continue;
    }
    // Prose after a heading or a list row opens a new block; prose after prose
    // is the same paragraph, blank lines included.
    if (boundaries.last !== "prose") settle(boundaries, start);
    boundaries.last = "prose";
  }
}

/** A parser that reuses the blocks an append-only text already settled. */
export interface SegmentCache {
  /**
   * The same segments `segmentsOf(text)` returns, reusing what the previous call
   * settled when `text` only grew. Any other change (a replacement, a truncation,
   * another turn's text) restarts the parse.
   */
  segmentsFor(text: string): TextSegment[];
}

/**
 * Creates a parser for one growing message — a streaming answer, or a bubble
 * whose text is re-rendered by the throttle.
 *
 * Re-parsing the whole message per delta is what this exists to avoid: a long
 * answer re-reads every block it already painted, once per delta, for no change
 * at all. Only the region after the last settled block can still change, so only
 * that region is parsed again; the blocks above it are handed back as the same
 * objects. The boundary is a line where `segmentsOf` would start a fresh block
 * (see `scanBoundaries`), so a fenced answer and a plain formatted one are both
 * reused: the fences are not the only constructs a stream settles.
 *
 * The reuse is a *composition*, not a second parser: the settled prefix and the
 * open tail are each parsed by `segmentsOf`, split at a line boundary that the
 * parser itself would have started a fresh region at, which is why the result is
 * exactly what one full parse produces.
 */
export function createSegmentCache(): SegmentCache {
  /** The text of the previous call; `null` before the first one. */
  let parsed: string | null = null;
  /** The incremental search for boundaries, kept across the appends. */
  let boundaries = freshBoundaries();
  /** The segments of `parsed.slice(0, boundaries.settled)`, reused verbatim. */
  let settledSegments: TextSegment[] = [];
  /** What `segmentsFor(parsed)` answered. */
  let answer: TextSegment[] = [];

  return {
    segmentsFor(text: string): TextSegment[] {
      if (parsed === text) return answer;
      // Anything but an append invalidates every offset the scan remembers.
      if (parsed === null || !text.startsWith(parsed)) {
        boundaries = freshBoundaries();
        settledSegments = [];
      }
      const wasSettled = boundaries.settled;
      scanBoundaries(boundaries, text);
      const settledAt = boundaries.settled;
      if (settledAt > wasSettled) {
        // Only the newly settled lines are read: the blocks above them stay.
        settledSegments = settledSegments.concat(
          segmentsOf(text.slice(wasSettled, settledAt)),
        );
      }
      const tail = segmentsOf(text.slice(settledAt));
      answer = settledAt === 0 ? tail : settledSegments.concat(tail);
      parsed = text;
      return answer;
    },
  };
}
