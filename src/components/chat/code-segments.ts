// Splitting assistant prose into text and fenced code, without a markdown
// dependency.
//
// pi streams plain text with three constructs the transcript honours: fenced
// code (``` optionally followed by a language), inline `code` spans, and line
// breaks. Everything else is prose and stays exactly as it arrived — the view
// renders every piece through interpolations, so user content is escaped by Vue
// itself and no shape here can ever become markup.
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

/** One piece of a message: Prose with line breaks, or a fenced code block. */
export type TextSegment =
  | { type: "text"; parts: InlinePart[] }
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
 * A prose region, split around its inline code and emphasis. A newline that
 * merely separated prose from a fence (or ended the message) is structural and
 * dropped, so an ordinary paragraph does not gain a blank line next to its code
 * block.
 */
function textSegment(raw: string): TextSegment | null {
  if (raw === "") return null;
  const parts: InlinePart[] = [];
  let last = 0;
  for (const match of raw.matchAll(INLINE_MARKDOWN)) {
    const at = match.index ?? 0;
    if (at > last) parts.push({ type: "plain", text: raw.slice(last, at) });
    parts.push(partOf(match));
    last = at + match[0].length;
  }
  if (last < raw.length) parts.push({ type: "plain", text: raw.slice(last) });
  if (parts.length === 0) return null;
  return { type: "text", parts };
}

/** Cuts the newline that ends a prose region sitting in front of a fence. */
function trimTrailingNewline(raw: string): string {
  return raw.endsWith("\n") ? raw.slice(0, -1) : raw;
}

/**
 * Segments one message: prose and fenced code, in the order they were written.
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

    // Everything before the fence is prose.
    const prose = textSegment(trimTrailingNewline(text.slice(proseStart, line.start)));
    if (prose !== null) segments.push(prose);

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

  const tail = textSegment(trimTrailingNewline(text.slice(proseStart)));
  if (tail !== null) segments.push(tail);
  return segments;
}
