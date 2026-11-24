/**
 * Pure text preparation & segmentation helpers for local CPU TTS.
 *
 * - Strips Markdown formatting, fenced code blocks, raw URLs, and Unicode emojis.
 * - Normalizes common tech/brand terms & acronyms so Spanish/English TTS
 *   pronounces them naturally (e.g. "Echo" -> "Eko", "ChatGPT" -> "Chat gi pi ti").
 * - Keeps the first synthesized segment short (~160 chars) so playback begins
 *   almost immediately, while subsequent segments (~320 chars) keep prosody
 *   natural and reduce ONNX invocation overhead.
 */

export interface SplitTextOptions {
  firstMaxChars?: number;
  maxChars?: number;
}

export interface PrepareSpeechOptions {
  /** Target TTS voice language (e.g. `"es"`, `"en-US"`). Defaults to `"es"`. */
  language?: string | null;
  /** Strip Unicode emojis & pictographs so TTS never reads them. Defaults to `true`. */
  stripEmojis?: boolean;
  /** Apply pronunciation dictionary rules. Defaults to `true`. */
  normalizePronunciation?: boolean;
}

const DEFAULT_FIRST_MAX = 160;
const DEFAULT_MAX = 320;
const MAX_SPEECH_CHARS = 5000;

interface PronunciationRule {
  pattern: RegExp;
  replacement: string;
}

function wordRule(term: string, replacement: string, caseSensitive = false): PronunciationRule {
  const flags = caseSensitive ? "gu" : "giu";
  return {
    pattern: new RegExp(`(?<![\\p{L}\\p{N}])${term}(?![\\p{L}\\p{N}])`, flags),
    replacement,
  };
}

const ES_PRONUNCIATION_RULES: PronunciationRule[] = [
  // Multi-word / compound terms first
  wordRule("Hugging\\s+Face", "Jaguin Feis"),
  wordRule("ChatGPT", "Chat gi pi ti"),
  wordRule("OpenAI", "Open ei ai"),
  wordRule("DeepSeek", "Dip Sik"),
  wordRule("TypeScript", "Taip Script"),
  wordRule("JavaScript", "Yava Script"),
  wordRule("GitHub", "Git Jab"),
  wordRule("GitLab", "Git Lab"),
  wordRule("SQLite", "ese cu lait"),
  wordRule("WhatsApp", "Guasap"),
  wordRule("YouTube", "Yutub"),
  wordRule("MacBook", "Macbuk"),
  wordRule("macOS", "mac o ese"),
  wordRule("iPhone", "Aifon"),
  wordRule("iPad", "Aipad"),
  wordRule("iOS", "ai o ese"),
  wordRule("Wi-?Fi", "guifi"),
  wordRule("Bluetooth", "blutuz"),
  // Single words & brand names
  wordRule("Echo", "Eko"),
  wordRule("Claude", "Clod"),
  wordRule("Anthropic", "Anzrópic"),
  wordRule("Python", "Paiton"),
  wordRule("Rust", "Rast"),
  wordRule("Vue", "Viu"),
  wordRule("React", "Riact"),
  wordRule("Apple", "Apel"),
  wordRule("Google", "Guguel"),
  wordRule("ONNX", "ónix"),
  wordRule("JSON", "yeison"),
  wordRule("YAML", "yamel"),
  // Acronyms (case-sensitive where lowercase could collide with real Spanish words)
  wordRule("GPT", "gi pi ti"),
  wordRule("LLMs", "ele ele emes", true),
  wordRule("LLM", "ele ele eme", true),
  wordRule("MCP", "eme ce pe", true),
  wordRule("CLI", "ce ele i", true),
  wordRule("TTS", "te te ese", true),
  wordRule("STT", "ese te te", true),
  wordRule("CPU", "ce pe u", true),
  wordRule("GPU", "ge pe u", true),
  wordRule("SQL", "ese cu ele", true),
  wordRule("HTML", "ache te eme ele", true),
  wordRule("CSS", "ce ese ese", true),
  wordRule("HTTPS", "ache te te pe ese", true),
  wordRule("HTTP", "ache te te pe", true),
  wordRule("URLs", "u erre eles", true),
  wordRule("URL", "u erre ele", true),
  wordRule("SSH", "ese ese ache", true),
  wordRule("AI", "ei ai", true),
  wordRule("IA", "i a", true),
];

const EN_PRONUNCIATION_RULES: PronunciationRule[] = [
  wordRule("ChatGPT", "Chat G P T"),
  wordRule("OpenAI", "Open A I"),
  wordRule("macOS", "mac O S"),
  wordRule("SQLite", "S Q Lite"),
  wordRule("ONNX", "onyx"),
  wordRule("MCP", "M C P", true),
  wordRule("CLI", "C L I", true),
  wordRule("TTS", "T T S", true),
  wordRule("STT", "S T T", true),
  wordRule("LLMs", "L L Ms", true),
  wordRule("LLM", "L L M", true),
];

/**
 * Removes Unicode emojis, pictographs, flags, skin-tone modifiers, and
 * variation selectors so local TTS engines never speak or choke on them.
 */
export function stripEmojisFromText(text: string): string {
  return text
    .replace(
      /(?:[\u{1F1E6}-\u{1F1FF}]{2}|[0-9#*]\uFE0F?\u20E3|\p{Extended_Pictographic}(?:\uFE0F|\uFE0E|[\u{1F3FB}-\u{1F3FF}]|\u200D\p{Extended_Pictographic})*)/gu,
      " ",
    )
    .replace(/[\uFE0E\uFE0F\u200D\u20E3]/g, "");
}

/**
 * Rewrites known brand names and acronyms into phonetically natural forms
 * for the target TTS language.
 */
export function applyPronunciationRules(text: string, language?: string | null): string {
  const isEnglish = (language ?? "").trim().toLowerCase().startsWith("en");
  const rules = isEnglish ? EN_PRONUNCIATION_RULES : ES_PRONUNCIATION_RULES;
  let out = text;
  for (const rule of rules) {
    out = out.replace(rule.pattern, rule.replacement);
  }
  return out;
}

/**
 * Strips Markdown formatting, fenced code blocks, raw URLs, emojis, and
 * applies pronunciation normalization so TTS reads natural prose.
 */
export function prepareTextForSpeech(
  raw: string,
  options: PrepareSpeechOptions = {},
): string {
  if (!raw) return "";
  let text = raw
    // Remove fenced code blocks completely
    .replace(/```[\s\S]*?```/g, " ")
    // Unwrap inline code backticks
    .replace(/`([^`]+)`/g, "$1")
    // Convert markdown links [label](url) -> label
    .replace(/\[([^\]]+)\]\(([^)]+)\)/g, "$1")
    // Remove markdown images ![alt](url)
    .replace(/!\[([^\]]*)\]\(([^)]+)\)/g, "$1")
    // Drop bare URLs
    .replace(/https?:\/\/\S+/g, " ")
    // Strip markdown heading / list / blockquote markers at line starts
    .replace(/^\s{0,3}(?:#{1,6}\s+|[-*+]\s+|\d+\.\s+|>\s+)/gm, "")
    // Strip bold / italic / strikethrough markers
    .replace(/(\*\*|__|\*|_|~~)/g, "");

  if (options.stripEmojis !== false) {
    text = stripEmojisFromText(text);
  }
  if (options.normalizePronunciation !== false) {
    text = applyPronunciationRules(text, options.language);
  }

  // Collapse whitespace and tidy spaces before punctuation left by stripped emojis/URLs
  text = text
    .replace(/\s+/g, " ")
    .replace(/\s+([.,!?;:])/g, "$1")
    .trim();

  if (text.length > MAX_SPEECH_CHARS) {
    text = text.slice(0, MAX_SPEECH_CHARS).trim();
  }
  return text;
}

function isLowSurrogate(code: number): boolean {
  return code >= 0xdc00 && code <= 0xdfff;
}

function safeCutIndex(text: string, limit: number): number {
  if (limit >= text.length) return text.length;
  let cut = limit;
  if (cut > 0 && isLowSurrogate(text.charCodeAt(cut))) {
    cut -= 1;
  }
  return Math.max(1, cut);
}

/**
 * Splits prepared text into non-empty chunks respecting sentence boundaries
 * first, then word boundaries, and finally UTF-16-safe hard cuts.
 */
export function splitTextForSpeech(
  text: string,
  options: SplitTextOptions = {},
): string[] {
  const cleaned = text.trim();
  if (!cleaned) return [];

  const firstMax = Math.max(20, options.firstMaxChars ?? DEFAULT_FIRST_MAX);
  const laterMax = Math.max(40, options.maxChars ?? DEFAULT_MAX);

  const segments: string[] = [];
  let cursor = 0;

  while (cursor < cleaned.length) {
    while (cursor < cleaned.length && /\s/.test(cleaned[cursor])) {
      cursor += 1;
    }
    if (cursor >= cleaned.length) break;

    const maxLen = segments.length === 0 ? firstMax : laterMax;
    const remaining = cleaned.length - cursor;
    if (remaining <= maxLen) {
      const tail = cleaned.slice(cursor).trim();
      if (tail) segments.push(tail);
      break;
    }

    const windowEnd = cursor + safeCutIndex(cleaned.slice(cursor), maxLen);
    const slice = cleaned.slice(cursor, windowEnd);

    // 1. Prefer the last sentence ending followed by whitespace
    let splitOffset = -1;
    const sentenceMatches = [...slice.matchAll(/[.!?…](?:\s+|$)/g)];
    if (sentenceMatches.length > 0) {
      const last = sentenceMatches[sentenceMatches.length - 1];
      if (last.index !== undefined && last.index >= 24) {
        splitOffset = last.index + 1;
      }
    }

    // 2. Fall back to the last whitespace before the limit
    if (splitOffset === -1) {
      const lastSpace = slice.search(/\s+\S*$/);
      if (lastSpace > 0) {
        splitOffset = lastSpace;
      }
    }

    // 3. Hard cut
    if (splitOffset <= 0) {
      splitOffset = slice.length;
    }

    const chunk = cleaned.slice(cursor, cursor + splitOffset).trim();
    if (chunk) segments.push(chunk);
    cursor += splitOffset;
  }

  return segments;
}
