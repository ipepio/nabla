/**
 * Pure Markdown <-> Notion-style block model for Nabla's Document tool.
 *
 * Keeps `.md` files on disk 100% standard Markdown while giving the UI a
 * single-view block editing experience (headings H1-H4, lists, to-dos, quotes,
 * code blocks, editable tables, inline local images, and dividers).
 */

export type TextBlockKind =
  | "paragraph"
  | "h1"
  | "h2"
  | "h3"
  | "h4"
  | "bullet"
  | "numbered"
  | "todo"
  | "quote"
  | "code";

export type DocBlock =
  | { id: string; kind: Exclude<TextBlockKind, "todo" | "code">; text: string }
  | { id: string; kind: "todo"; text: string; checked: boolean }
  | { id: string; kind: "code"; text: string; lang: string }
  | { id: string; kind: "table"; headers: string[]; rows: string[][] }
  | { id: string; kind: "image"; alt: string; src: string }
  | { id: string; kind: "divider" };

let blockSeq = 0;

export function nextBlockId(): string {
  blockSeq += 1;
  return `blk-${blockSeq}`;
}

export function createTextBlock(
  kind: TextBlockKind = "paragraph",
  text = "",
): DocBlock {
  if (kind === "todo") {
    return { id: nextBlockId(), kind: "todo", text, checked: false };
  }
  if (kind === "code") {
    return { id: nextBlockId(), kind: "code", text, lang: "" };
  }
  return { id: nextBlockId(), kind, text };
}

export function isTextBlock(
  block: DocBlock,
): block is Extract<DocBlock, { text: string }> {
  return (
    block.kind !== "table" &&
    block.kind !== "image" &&
    block.kind !== "divider"
  );
}

/**
 * Detects Notion-style Markdown prefix shortcuts typed at the start of a
 * paragraph (e.g. `# `, `## `, `### `, `#### `, `- `, `1. `, `[] `, `> `, ```` ``` ````).
 */
export function detectMarkdownShortcut(
  raw: string,
): { kind: TextBlockKind | "divider"; rest: string } | null {
  if (raw === "---" || raw === "***") {
    return { kind: "divider", rest: "" };
  }
  if (raw.startsWith("#### ")) return { kind: "h4", rest: raw.slice(5) };
  if (raw.startsWith("### ")) return { kind: "h3", rest: raw.slice(4) };
  if (raw.startsWith("## ")) return { kind: "h2", rest: raw.slice(3) };
  if (raw.startsWith("# ")) return { kind: "h1", rest: raw.slice(2) };
  if (raw.startsWith("[] ") || raw.startsWith("[ ] ")) {
    return { kind: "todo", rest: raw.slice(raw.indexOf(" ") + 1) };
  }
  if (raw.startsWith("- [ ] ")) return { kind: "todo", rest: raw.slice(6) };
  if (raw.startsWith("- ") || raw.startsWith("* ")) {
    return { kind: "bullet", rest: raw.slice(2) };
  }
  const numMatch = /^\d+\.\s+(.*)$/.exec(raw);
  if (numMatch) return { kind: "numbered", rest: numMatch[1] };
  if (raw.startsWith("> ")) return { kind: "quote", rest: raw.slice(2) };
  if (raw === "```" || raw.startsWith("``` ")) {
    return { kind: "code", rest: raw.slice(3).trimStart() };
  }
  return null;
}

function splitTableRow(line: string): string[] {
  return line
    .trim()
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split("|")
    .map((cell) => cell.trim());
}

/** Parses standard Markdown into an ordered list of editable Notion-style blocks. */
export function parseMarkdownToBlocks(markdown: string): DocBlock[] {
  const lines = markdown.replace(/\r\n/g, "\n").split("\n");
  const blocks: DocBlock[] = [];
  let i = 0;

  while (i < lines.length) {
    const raw = lines[i];
    const trimmed = raw.trim();

    if (!trimmed) {
      i += 1;
      continue;
    }

    if (trimmed.startsWith("```")) {
      const lang = trimmed.slice(3).trim();
      i += 1;
      const codeLines: string[] = [];
      while (i < lines.length && !lines[i].trim().startsWith("```")) {
        codeLines.push(lines[i]);
        i += 1;
      }
      if (i < lines.length) i += 1;
      blocks.push({
        id: nextBlockId(),
        kind: "code",
        lang,
        text: codeLines.join("\n"),
      });
      continue;
    }

    if (/^(-{3,}|\*{3,}|_{3,})$/.test(trimmed)) {
      blocks.push({ id: nextBlockId(), kind: "divider" });
      i += 1;
      continue;
    }

    const imgMatch = /^!\[([^\]]*)\]\(([^)]+)\)$/.exec(trimmed);
    if (imgMatch) {
      blocks.push({
        id: nextBlockId(),
        kind: "image",
        alt: imgMatch[1].trim(),
        src: imgMatch[2].trim().replace(/^<|>$/g, ""),
      });
      i += 1;
      continue;
    }

    const headingMatch = /^(#{1,4})\s+(.*)$/.exec(trimmed);
    if (headingMatch) {
      const level = headingMatch[1].length;
      const kind = (`h${level}`) as "h1" | "h2" | "h3" | "h4";
      blocks.push({
        id: nextBlockId(),
        kind,
        text: headingMatch[2].trim(),
      });
      i += 1;
      continue;
    }

    if (trimmed.startsWith(">")) {
      const qLines: string[] = [];
      while (i < lines.length && lines[i].trim().startsWith(">")) {
        qLines.push(lines[i].trim().replace(/^>\s?/, ""));
        i += 1;
      }
      blocks.push({
        id: nextBlockId(),
        kind: "quote",
        text: qLines.join("\n"),
      });
      continue;
    }

    if (
      trimmed.startsWith("|") &&
      i + 1 < lines.length &&
      /^\|?[\s:|-]+\|?$/.test(lines[i + 1].trim()) &&
      lines[i + 1].includes("-")
    ) {
      const headers = splitTableRow(trimmed);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && lines[i].trim().startsWith("|")) {
        const row = splitTableRow(lines[i]);
        while (row.length < headers.length) row.push("");
        rows.push(row.slice(0, headers.length));
        i += 1;
      }
      blocks.push({
        id: nextBlockId(),
        kind: "table",
        headers,
        rows: rows.length > 0 ? rows : [headers.map(() => "")],
      });
      continue;
    }

    const todoMatch = /^[-*+]\s+\[([ xX])\]\s*(.*)$/.exec(trimmed);
    if (todoMatch) {
      blocks.push({
        id: nextBlockId(),
        kind: "todo",
        checked: todoMatch[1].toLowerCase() === "x",
        text: todoMatch[2],
      });
      i += 1;
      continue;
    }

    if (/^[-*+]\s+/.test(trimmed)) {
      blocks.push({
        id: nextBlockId(),
        kind: "bullet",
        text: trimmed.replace(/^[-*+]\s+/, ""),
      });
      i += 1;
      continue;
    }

    if (/^\d+\.\s+/.test(trimmed)) {
      blocks.push({
        id: nextBlockId(),
        kind: "numbered",
        text: trimmed.replace(/^\d+\.\s+/, ""),
      });
      i += 1;
      continue;
    }

    const paraLines: string[] = [];
    while (
      i < lines.length &&
      lines[i].trim() !== "" &&
      !/^(#{1,4}\s|```|>|[-*+]\s|\d+\.\s|!\[|\||(-{3,}|\*{3,}|_{3,})$)/.test(
        lines[i].trim(),
      )
    ) {
      paraLines.push(lines[i]);
      i += 1;
    }
    blocks.push({
      id: nextBlockId(),
      kind: "paragraph",
      text: paraLines.join("\n"),
    });
  }

  return blocks.length > 0 ? blocks : [createTextBlock("h1", "")];
}

/** Serializes Notion-style blocks back into clean, standard Markdown. */
export function serializeBlocksToMarkdown(blocks: readonly DocBlock[]): string {
  const out: string[] = [];
  let numberedIndex = 0;

  for (let i = 0; i < blocks.length; i += 1) {
    const block = blocks[i];
    const prev = i > 0 ? blocks[i - 1] : null;
    const sameListGroup =
      prev !== null &&
      ((block.kind === "bullet" && prev.kind === "bullet") ||
        (block.kind === "numbered" && prev.kind === "numbered") ||
        (block.kind === "todo" && prev.kind === "todo"));

    if (block.kind === "numbered") {
      numberedIndex = prev?.kind === "numbered" ? numberedIndex + 1 : 1;
    } else {
      numberedIndex = 0;
    }

    let chunk = "";
    switch (block.kind) {
      case "h1":
        chunk = `# ${block.text}`;
        break;
      case "h2":
        chunk = `## ${block.text}`;
        break;
      case "h3":
        chunk = `### ${block.text}`;
        break;
      case "h4":
        chunk = `#### ${block.text}`;
        break;
      case "paragraph":
        chunk = block.text;
        break;
      case "bullet":
        chunk = `- ${block.text}`;
        break;
      case "numbered":
        chunk = `${numberedIndex}. ${block.text}`;
        break;
      case "todo":
        chunk = `- [${block.checked ? "x" : " "}] ${block.text}`;
        break;
      case "quote":
        chunk = block.text
          .split("\n")
          .map((line) => `> ${line}`)
          .join("\n");
        break;
      case "code":
        chunk = `\`\`\`${block.lang}\n${block.text}\n\`\`\``;
        break;
      case "divider":
        chunk = "---";
        break;
      case "image":
        chunk = `![${block.alt}](${block.src})`;
        break;
      case "table": {
        const headerLine = `| ${block.headers.join(" | ")} |`;
        const sepLine = `| ${block.headers.map(() => "---").join(" | ")} |`;
        const rowLines = block.rows.map((r) => `| ${r.join(" | ")} |`);
        chunk = [headerLine, sepLine, ...rowLines].join("\n");
        break;
      }
    }

    if (out.length === 0) {
      out.push(chunk);
    } else if (sameListGroup) {
      out[out.length - 1] = `${out[out.length - 1]}\n${chunk}`;
    } else {
      out.push(chunk);
    }
  }

  return `${out.join("\n\n").trimEnd()}\n`;
}
