// Equivalence and reuse of the segment cache (`src/components/chat/code-segments.ts`).
//
// The parser is the contract: every sequence of calls on the cache must return
// exactly what `segmentsOf` returns for that text. What the cache may change is
// *how* it gets there: a streamed answer only appends, so the blocks that ended
// before the append are reused instead of re-parsed.
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  createSegmentCache,
  segmentsOf,
} from "../src/components/chat/code-segments.ts";

/** Texts that exercise every construct the parser knows, plus the edge cases. */
const FIXTURES = [
  "",
  "hello",
  "hello\n",
  "one paragraph\n\nanother paragraph",
  "# heading\n- one\n- two\n",
  "prose\n### heading without closing\n",
  "```ts\nconst a = 1;\n```\n",
  "```ts\nconst a = 1;\n```\nafter the block",
  "prose before\n```js\nopen fence",
  "```\na\n```\n```\nb\n```\n",
  "text\n```js\nx\n```\n1. one\n2. two\n```\nunclosed",
  "```\n```\n```\n```\n",
  // Plain formatted answers: no fence anywhere, so every boundary the cache
  // uses has to come from a heading, a list run or the prose↔list transition.
  "# Plan\n- one\n- two\nClosing prose\nand more",
  "intro\n\n## Step\n\n1. first\n2. second\n\ntail\n",
  "- a\n\n- b",
  "prose\n- a\n1. b\nprose again\n",
  "# h\n\n\n# i\n",
  "  - indented row\n\t* tabbed row\n",
];

/**
 * Deterministic generator: the equivalence claim has to hold for text nobody
 * thought of, and a fixed seed keeps a failure reproducible.
 */
function pseudoRandom(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const ALPHABET = "```\n\n #-*+1.`_[]()abc  \t";

test("every prefix of every fixture parses exactly as segmentsOf does", () => {
  for (const fixture of FIXTURES) {
    const cache = createSegmentCache();
    for (let end = 0; end <= fixture.length; end += 1) {
      const prefix = fixture.slice(0, end);
      assert.deepEqual(
        cache.segmentsFor(prefix),
        segmentsOf(prefix),
        `prefix ${JSON.stringify(prefix)} of ${JSON.stringify(fixture)}`,
      );
    }
  }
});

test("random append-only streams stay byte-identical to segmentsOf", () => {
  const random = pseudoRandom(0x5eed);
  for (let run = 0; run < 60; run += 1) {
    const cache = createSegmentCache();
    let text = "";
    const steps = 1 + Math.floor(random() * 12);
    for (let step = 0; step < steps; step += 1) {
      const chunk = Array.from(
        { length: 1 + Math.floor(random() * 6) },
        () => ALPHABET[Math.floor(random() * ALPHABET.length)] ?? "",
      ).join("");
      text += chunk;
      assert.deepEqual(
        cache.segmentsFor(text),
        segmentsOf(text),
        `after appending ${JSON.stringify(chunk)} → ${JSON.stringify(text)}`,
      );
    }
  }
});

/** Lines that move the plain prose parser between blocks, blank and fences. */
const LINE_POOL = [
  "",
  "prose line",
  "more prose",
  "# heading",
  "## deeper",
  "#hashtag",
  "- bullet",
  "* star",
  "+ plus",
  "- again",
  "1. first",
  "2. second",
  "  indented",
  "```",
  "```js",
];

test("random line streams stay byte-identical to segmentsOf", () => {
  const random = pseudoRandom(0xc0ffee);
  for (let run = 0; run < 60; run += 1) {
    const cache = createSegmentCache();
    let text = "";
    const steps = 1 + Math.floor(random() * 10);
    for (let step = 0; step < steps; step += 1) {
      const line = LINE_POOL[Math.floor(random() * LINE_POOL.length)] ?? "";
      // Most lines are terminated, so the scan sees complete lines; the rest
      // leave the last line growing, which is the streaming case.
      text += random() < 0.8 ? `${line}\n` : line;
      assert.deepEqual(
        cache.segmentsFor(text),
        segmentsOf(text),
        `after appending a line → ${JSON.stringify(text)}`,
      );
    }
  }
});

test("plain formatted blocks are reused while the prose keeps streaming", () => {
  const cache = createSegmentCache();
  const first = cache.segmentsFor("# Plan\n- one\n- two\nClosing prose\n");
  const grown = cache.segmentsFor("# Plan\n- one\n- two\nClosing prose\nand more");
  assert.deepEqual(first, segmentsOf("# Plan\n- one\n- two\nClosing prose\n"));
  assert.deepEqual(
    grown,
    segmentsOf("# Plan\n- one\n- two\nClosing prose\nand more"),
  );
  // No fence anywhere: the settled blocks are a heading and a list run, and
  // object identity is the observable proof that they were not parsed again.
  assert.equal(grown.length, 3);
  assert.equal(grown[0], first[0]);
  assert.equal(grown[1], first[1]);
});

test("a growing paragraph is one block, never two", () => {
  const cache = createSegmentCache();
  assert.deepEqual(cache.segmentsFor("first line\n"), segmentsOf("first line\n"));
  const grown = cache.segmentsFor("first line\nsecond line\n");
  // A line boundary inside a paragraph is not a block boundary: splitting there
  // would invent a block the parser never produced.
  assert.equal(grown.length, 1);
  assert.deepEqual(grown, segmentsOf("first line\nsecond line\n"));
});

test("a settled paragraph is reused when a block starts after it", () => {
  const cache = createSegmentCache();
  const first = cache.segmentsFor("first prose\n# heading\n");
  const grown = cache.segmentsFor("first prose\n# heading\nand the tail");
  assert.deepEqual(first, segmentsOf("first prose\n# heading\n"));
  assert.deepEqual(
    grown,
    segmentsOf("first prose\n# heading\nand the tail"),
  );
  assert.equal(grown.length, 3);
  assert.equal(grown[0], first[0]);
  // The heading is where the append landed: only the paragraph above it settled.
  assert.notEqual(grown[1], first[1]);
});

test("an open list run is not split, so its rows stay one block", () => {
  const cache = createSegmentCache();
  const first = cache.segmentsFor("# Plan\n- one\n");
  const grown = cache.segmentsFor("# Plan\n- one\n- two\n");
  // `- two` extends the run `- one` opened: reusing a list of one would have
  // invented a block boundary the parser never saw.
  assert.deepEqual(grown, segmentsOf("# Plan\n- one\n- two\n"));
  assert.equal(grown[0], first[0]);
  assert.equal(grown[1] === first[1], false);
  assert.equal(grown[1]?.type === "list" ? grown[1].items.length : null, 2);
});

test("a closed fence is reused when the answer keeps growing", () => {
  const cache = createSegmentCache();
  const first = cache.segmentsFor("```ts\nconst a = 1;\n```\n");
  const grown = cache.segmentsFor("```ts\nconst a = 1;\n```\nand then the prose");
  assert.equal(grown.length, 2);
  // Object identity is the observable proof that the block was not re-parsed.
  assert.equal(grown[0], first[0]);
  assert.deepEqual(grown, segmentsOf("```ts\nconst a = 1;\n```\nand then the prose"));
});

test("the last closed fence of a longer answer is the reuse boundary", () => {
  const cache = createSegmentCache();
  const first = cache.segmentsFor("intro\n```js\nlet a = 1;\n```\n");
  const grown = cache.segmentsFor("intro\n```js\nlet a = 1;\n```\nmore ```js\nlet b");
  assert.deepEqual(grown, segmentsOf("intro\n```js\nlet a = 1;\n```\nmore ```js\nlet b"));
  assert.equal(grown[1], first[1]);
});

test("a fence still open is never treated as finalized", () => {
  const cache = createSegmentCache();
  const first = cache.segmentsFor("```js\nlet a = 1;");
  assert.deepEqual(first, segmentsOf("```js\nlet a = 1;"));
  // Closing it changes the block's `closed` flag and its source: a cached block
  // would have been a lie here.
  const closed = cache.segmentsFor("```js\nlet a = 1;\n```\n");
  assert.deepEqual(closed, segmentsOf("```js\nlet a = 1;\n```\n"));
  assert.equal(closed[0]?.type === "code" ? closed[0].closed : null, true);
});

test("a closing fence appended to the last line does not reuse the old block", () => {
  const cache = createSegmentCache();
  // The closing line has no newline yet, so the next append can still rewrite it.
  const first = cache.segmentsFor("```\ncode\n```");
  const grown = cache.segmentsFor("```\ncode\n```x");
  assert.deepEqual(first, segmentsOf("```\ncode\n```"));
  assert.deepEqual(grown, segmentsOf("```\ncode\n```x"));
  // ` ```x ` is an opening fence with the language `x`, not a closing one.
  assert.equal(grown[0]?.type === "code" ? grown[0].closed : null, false);
});

test("a replaced or truncated text restarts the parse", () => {
  const cache = createSegmentCache();
  cache.segmentsFor("```js\nlet a = 1;\n```\ntail\n\nmore prose\n\nand more");
  const replaced = cache.segmentsFor("totally different");
  assert.deepEqual(replaced, segmentsOf("totally different"));
  const truncated = cache.segmentsFor("```js\nlet a");
  assert.deepEqual(truncated, segmentsOf("```js\nlet a"));
});

test("asking twice for the same text answers the same segments", () => {
  const cache = createSegmentCache();
  const once = cache.segmentsFor("a\n```\nb\n```\nc");
  const twice = cache.segmentsFor("a\n```\nb\n```\nc");
  assert.deepEqual(twice, once);
  assert.equal(twice[1], once[1]);
});
