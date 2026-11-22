import assert from "node:assert/strict";
import test from "node:test";
import { formatTranscriptMarkdown } from "../src/components/chat/export-markdown.ts";
import {
  contextUsageRatio,
  estimateTokens,
  formatCompactTokens,
  formatElapsed,
} from "../src/components/chat/session-metrics.ts";
import {
  createSegmentCache,
  segmentsOf,
} from "../src/components/chat/code-segments.ts";

test("formatTranscriptMarkdown renders headings, workspace metadata and non-empty turns", () => {
  const md = formatTranscriptMarkdown("nabla", "API review", [
    { id: "1", role: "user", text: "Check the seam", at: "10:01" },
    { id: "2", role: "assistant", text: "   ", at: null },
    { id: "3", role: "assistant", text: "Looks clean.", at: "10:02" },
  ]);
  assert.match(md, /^# API review\n/);
  assert.match(md, /> Workspace: `nabla`/);
  assert.match(md, /## User \(10:01\)\n\nCheck the seam/);
  assert.match(md, /## Nabla \(10:02\)\n\nLooks clean\./);
});

test("session metrics helpers format tokens, elapsed time and context ratio", () => {
  assert.equal(estimateTokens(["abcd", "efgh"]), 2);
  assert.equal(formatCompactTokens(420), "420");
  assert.equal(formatCompactTokens(4200), "4.2k");
  assert.equal(formatCompactTokens(128000), "128k");
  assert.equal(formatElapsed(5), "00:05");
  assert.equal(formatElapsed(125), "02:05");
  assert.equal(formatElapsed(3661), "01:01:01");
  assert.equal(contextUsageRatio(500, 1000), 0.5);
  assert.equal(contextUsageRatio(500, null), null);
});

test("segmentsOf and createSegmentCache parse GFM tables, blockquotes and hr", () => {
  const md = [
    "### Divergencias",
    "",
    "| # | Dónde | Qué |",
    "|---|:---:|---:|",
    "| 1 | `error.rs` | Devuelve **CommandError** |",
    "| 2 | `contracts.rs` | Falta espejo TS |",
    "",
    "> Nota importante",
    "",
    "---",
  ].join("\n");

  const full = segmentsOf(md);
  const table = full.find((s) => s.type === "table");
  assert.ok(table && table.type === "table");
  assert.equal(table.headers.length, 3);
  assert.deepEqual(table.alignments, ["left", "center", "right"]);
  assert.equal(table.rows.length, 2);
  assert.ok(full.some((s) => s.type === "blockquote"));
  assert.ok(full.some((s) => s.type === "hr"));

  const cache = createSegmentCache();
  for (let i = 1; i <= md.length; i += 7) {
    cache.segmentsFor(md.slice(0, i));
  }
  assert.deepEqual(cache.segmentsFor(md), full);
});
