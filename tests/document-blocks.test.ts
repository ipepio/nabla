import test from "node:test";
import assert from "node:assert/strict";
import {
  detectMarkdownShortcut,
  parseMarkdownToBlocks,
  serializeBlocksToMarkdown,
} from "../src/components/tools/document-blocks.ts";

test("parseMarkdownToBlocks and serializeBlocksToMarkdown round-trip Notion blocks", () => {
  const md = [
    "# Main Title",
    "",
    "#### Deep Heading",
    "",
    "A plain paragraph.",
    "",
    "- First bullet",
    "- Second bullet",
    "",
    "1. Step one",
    "2. Step two",
    "",
    "- [ ] Pending task",
    "- [x] Completed task",
    "",
    "> Important quote",
    "",
    "| Name | Score |",
    "| --- | --- |",
    "| Alice | 10 |",
    "",
    "![Diagram](diagram.png)",
    "",
    "---",
    "",
    "```ts",
    "const x = 1;",
    "```",
    "",
  ].join("\n");

  const blocks = parseMarkdownToBlocks(md);
  assert.deepEqual(
    blocks.map((b) => b.kind),
    [
      "h1",
      "h4",
      "paragraph",
      "bullet",
      "bullet",
      "numbered",
      "numbered",
      "todo",
      "todo",
      "quote",
      "table",
      "image",
      "divider",
      "code",
    ],
  );
  assert.equal(serializeBlocksToMarkdown(blocks), md);
});

test("detectMarkdownShortcut recognizes headings, lists, todo, quote, code and divider", () => {
  assert.deepEqual(detectMarkdownShortcut("# Hello"), { kind: "h1", rest: "Hello" });
  assert.deepEqual(detectMarkdownShortcut("#### Sub"), { kind: "h4", rest: "Sub" });
  assert.deepEqual(detectMarkdownShortcut("- Item"), { kind: "bullet", rest: "Item" });
  assert.deepEqual(detectMarkdownShortcut("1. First"), { kind: "numbered", rest: "First" });
  assert.deepEqual(detectMarkdownShortcut("[] Buy milk"), { kind: "todo", rest: "Buy milk" });
  assert.deepEqual(detectMarkdownShortcut("> Note"), { kind: "quote", rest: "Note" });
  assert.deepEqual(detectMarkdownShortcut("---"), { kind: "divider", rest: "" });
  assert.equal(detectMarkdownShortcut("Normal text"), null);
});
