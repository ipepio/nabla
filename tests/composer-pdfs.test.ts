// Behaviour of PDF input and turn composition (S3 of
// `odd/tasks/composer-tools-pdf.md`).
//
// Three modules own it and the last one decides what a turn really contains:
//
// - `src/components/chat/composer-pdfs.ts` holds the PDF rules — how many fit,
//   what "too large" means, which page is which, and what a scan is.
// - `src/components/chat/composer-turn.ts` composes the wire prompt (question +
//   requested tools + page-numbered document) and the label a transcript keeps.
//   Nothing else in the app builds that text.
// - `src/lib/composer-attachments.ts` holds the pending PDFs per draft key, in
//   memory only: session isolation, the move on the first send, and the "no
//   document text anywhere persistent" rule are what this file pins.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { registerHooks } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  COMPOSER_PDF_LIMIT,
  DEFAULT_PDF_LIMITS,
  addComposerPdf,
  hasUnreadablePdf,
  pdfsPromptBlock,
  removeComposerPdf,
  type ComposerPdf,
} from "../src/components/chat/composer-pdfs.ts";
import type { PdfAttachment } from "../src/contracts/types.ts";

const projectRoot = pathToFileURL(
  path.resolve(import.meta.dirname, "..") + path.sep,
).href;

// The app resolves extensionless relative imports and maps `@/` to `src/`;
// Node resolves neither, so both are mapped here as the bundler does.
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier.startsWith("@/")) {
      const mapped = new URL("src/" + specifier.slice(2), projectRoot);
      const target = existsSync(fileURLToPath(mapped))
        ? mapped.href
        : `${mapped.href}.ts`;
      return withJsonAttribute(nextResolve(target, context));
    }
    if (!path.extname(specifier) && specifier.startsWith(".")) {
      const candidate = new URL(specifier + ".ts", context.parentURL);
      if (existsSync(fileURLToPath(candidate))) {
        return withJsonAttribute(nextResolve(candidate.href, context));
      }
    }
    return withJsonAttribute(nextResolve(specifier, context));
  },
});

/**
 * Node refuses a JSON module import without `with { type: "json" }`, while the
 * bundler accepts the plain form `src/lib/i18n.ts` uses. The attribute is added
 * here, at resolution time, so application code keeps the shape Vite needs.
 */
function withJsonAttribute(resolved) {
  if (typeof resolved === "string") return resolved;
  if (!resolved.url.endsWith(".json")) return resolved;
  return { ...resolved, importAttributes: { type: "json" } };
}

const { composeTurnDisplay, composeTurnPrompt } = await import(
  "../src/components/chat/composer-turn.ts"
);

const {
  addPdfAttachment,
  clearAttachments,
  intentsFor,
  moveAttachments,
  pdfsFor,
  removePdfAttachment,
  toggleIntent,
} = await import("../src/lib/composer-attachments.ts");

/** What Rust answers for a text PDF: page order is page numbering. */
function attachment(
  name: string,
  pages: string[],
  overrides: Partial<PdfAttachment> = {},
): PdfAttachment {
  const text = pages.join("");
  return {
    name,
    pageCount: pages.length,
    pages,
    charCount: text.length,
    hasText: text.trim() !== "",
    byteLength: 2048,
    ...overrides,
  };
}

function accepted(candidate: PdfAttachment): ComposerPdf {
  const result = addComposerPdf([], candidate);
  assert.equal(result.rejection, null);
  assert.equal(result.pdfs.length, 1);
  return result.pdfs[0]!;
}

test("a PDF keeps its page numbers, including the pages that hold nothing", () => {
  const report = accepted(attachment("informe.pdf", ["Alpha", "", "Gamma"]));
  assert.equal(report.pageCount, 3);
  assert.equal(report.pages[1], "", "an empty page stays an empty page");
  assert.equal(
    pdfsPromptBlock([report]),
    [
      "--- PDF: informe.pdf (pages: 3) ---",
      "[page 1]",
      "Alpha",
      "[page 2]",
      "[page 3]",
      "Gamma",
      "--- end PDF: informe.pdf ---",
    ].join("\n"),
  );
  // Page 3 is still labelled 3: an empty page never renumbers the ones after it.
  assert.equal(pdfsPromptBlock([]), "");
});

test("a question with no attachment is byte for byte the text it was", () => {
  const parts = { imageCount: 0, pdfs: [], intents: [] };
  assert.equal(composeTurnPrompt("hola", parts), "hola");
  assert.equal(composeTurnDisplay("hola", parts), "hola");
  assert.equal(composeTurnPrompt("  hola  ", parts), "hola");
  assert.equal(composeTurnPrompt("", parts), "");
});

test("the document travels with the question, page by page", () => {
  const report = accepted(attachment("informe.pdf", ["Alpha", "Beta"]));
  const prompt = composeTurnPrompt("Resume este informe y dime en qué página aparece cada conclusión.", {
    imageCount: 0,
    pdfs: [report],
    intents: [],
  });
  assert.equal(
    prompt,
    [
      "Resume este informe y dime en qué página aparece cada conclusión.",
      "",
      "--- PDF: informe.pdf (pages: 2) ---",
      "[page 1]",
      "Alpha",
      "[page 2]",
      "Beta",
      "--- end PDF: informe.pdf ---",
    ].join("\n"),
  );
});

test("a PDF alone is a turn, and two PDFs keep their own page numbers", () => {
  const first = accepted(attachment("a.pdf", ["one"]));
  const second = accepted(attachment("b.pdf", ["two", ""]));
  const prompt = composeTurnPrompt("", { imageCount: 0, pdfs: [first, second], intents: [] });
  assert.equal(
    prompt,
    [
      "--- PDF: a.pdf (pages: 1) ---",
      "[page 1]",
      "one",
      "--- end PDF: a.pdf ---",
      "",
      "--- PDF: b.pdf (pages: 2) ---",
      "[page 1]",
      "two",
      "[page 2]",
      "--- end PDF: b.pdf ---",
    ].join("\n"),
  );
});

test("a turn carries at most the bounded number of PDFs and a rejection changes nothing", () => {
  const full = Array.from({ length: COMPOSER_PDF_LIMIT }, (_, index) => ({
    ...attachment(`doc-${index}.pdf`, ["text"]),
  }));
  let list: ComposerPdf[] = [];
  for (const candidate of full) {
    const result = addComposerPdf(list, candidate);
    assert.equal(result.rejection, null);
    list = result.pdfs;
  }
  assert.equal(list.length, COMPOSER_PDF_LIMIT);
  const overflow = addComposerPdf(list, attachment("one-too-many.pdf", ["text"]));
  assert.equal(overflow.rejection, "too-many");
  assert.deepEqual(
    overflow.pdfs.map((entry) => entry.name),
    list.map((entry) => entry.name),
    "a refused PDF leaves the attached ones exactly as they were",
  );
});

test("the page, character and size bounds refuse before anything is composed", () => {
  const tiny = { ...DEFAULT_PDF_LIMITS, pages: 2, chars: 8, count: 2 };
  assert.equal(addComposerPdf([], attachment("long.pdf", ["a", "b", "c"]), tiny).rejection, "too-many-pages");
  assert.equal(addComposerPdf([], attachment("wide.pdf", ["123456789"]), tiny).rejection, "too-large");
  assert.equal(addComposerPdf([], attachment("empty.pdf", []), tiny).rejection, "empty");
});

test("a scan is reported, never filled in: it attaches and blocks sending", () => {
  const scan = accepted(
    attachment("escaneado.pdf", ["", ""], { hasText: false }),
  );
  assert.equal(scan.hasText, false);
  assert.equal(hasUnreadablePdf([scan]), true);
  // No text is invented for it: the pages the extraction found are empty.
  assert.equal(pdfsPromptBlock([scan]), [
    "--- PDF: escaneado.pdf (pages: 2) ---",
    "[page 1]",
    "[page 2]",
    "--- end PDF: escaneado.pdf ---",
  ].join("\n"));
  assert.equal(hasUnreadablePdf([accepted(attachment("texto.pdf", ["hola"]))]), false);
  assert.equal(hasUnreadablePdf([]), false);
});

test("removing one PDF leaves the others exactly as they were", () => {
  const first = accepted(attachment("a.pdf", ["one"]));
  const second = accepted(attachment("b.pdf", ["two"]));
  assert.deepEqual(removeComposerPdf([first, second], first.id).map((entry) => entry.name), ["b.pdf"]);
  assert.deepEqual(removeComposerPdf([second], "unknown").map((entry) => entry.name), ["b.pdf"]);
});

test("the label a transcript keeps names the attachments, not their text", () => {
  const report = accepted(attachment("informe.pdf", ["Alpha", "Beta"]));
  const label = composeTurnDisplay("Resume esto", {
    imageCount: 2,
    pdfs: [report],
    intents: ["web-search"],
  });
  assert.equal(label, "Resume esto\nImages: 2\nPDF: informe.pdf\nTools: Web search");
  assert.equal(
    label.includes("Alpha"),
    false,
    "the extracted text never enters the transcript",
  );
  // A PDF-only turn still has a label of its own.
  assert.equal(
    composeTurnDisplay("", { imageCount: 0, pdfs: [report], intents: [] }),
    "PDF: informe.pdf",
  );
});

// PRESERVE: the label of a prompt that carries only images is what it always
// was (`appendImageLabel` in `composer-images.ts`), so the new composition never
// changes an existing image-only turn.
test("an image-only turn keeps the label it always had", () => {
  assert.equal(
    composeTurnDisplay("", { imageCount: 1, pdfs: [], intents: [] }),
    "Images: 1",
  );
  assert.equal(
    composeTurnDisplay("mira esto", { imageCount: 2, pdfs: [], intents: [] }),
    "mira esto\nImages: 2",
  );
});

test("pending PDFs belong to the draft key they were attached under", () => {
  const keyA = "nabla.draft.pdf-a";
  const keyB = "nabla.draft.pdf-b";
  clearAttachments(keyA);
  clearAttachments(keyB);

  assert.equal(addPdfAttachment(keyA, attachment("a.pdf", ["one"])).added, 1);
  assert.equal(addPdfAttachment(keyB, attachment("b.pdf", ["two"])).added, 1);
  assert.deepEqual(pdfsFor(keyA).map((entry) => entry.name), ["a.pdf"]);
  assert.deepEqual(pdfsFor(keyB).map((entry) => entry.name), ["b.pdf"]);
  assert.deepEqual(pdfsFor(null), []);

  // One conversation's undo never touches another's.
  removePdfAttachment(keyB, pdfsFor(keyA)[0]!.id);
  assert.equal(pdfsFor(keyA).length, 1);
  assert.equal(pdfsFor(keyB).length, 1);
  clearAttachments(keyA);
  assert.deepEqual(pdfsFor(keyA), []);
  assert.equal(pdfsFor(keyB).length, 1);

  clearAttachments(keyB);
});

test("the first send moves the pending PDF with the text and its images", () => {
  const workspaceKey = "nabla.draft.new.9";
  const sessionKey = "nabla.draft.90";
  clearAttachments(workspaceKey);
  clearAttachments(sessionKey);

  addPdfAttachment(workspaceKey, attachment("informe.pdf", ["Alpha"]));
  toggleIntent(workspaceKey, "browser");
  moveAttachments(workspaceKey, sessionKey);

  assert.deepEqual(pdfsFor(workspaceKey), []);
  assert.deepEqual(intentsFor(workspaceKey), []);
  assert.deepEqual(pdfsFor(sessionKey).map((entry) => entry.name), ["informe.pdf"]);
  assert.deepEqual(intentsFor(sessionKey), ["browser"]);
  clearAttachments(sessionKey);
});

test("a sent or queued turn is composed once, and the cleared draft composes nothing", () => {
  const key = "nabla.draft.once";
  clearAttachments(key);
  addPdfAttachment(key, attachment("informe.pdf", ["Alpha"]));
  toggleIntent(key, "web-search");

  const parts = {
    imageCount: 0,
    pdfs: [...pdfsFor(key)],
    intents: [...intentsFor(key)],
  };
  const first = composeTurnPrompt("Resume esto", parts);
  const second = composeTurnPrompt("Resume esto", parts);
  assert.equal(first, second, "composing does not consume the request");
  assert.equal(
    first.split("--- PDF: informe.pdf").length - 1,
    1,
    "one turn carries the document once",
  );
  assert.equal(
    first.split("[Requested tool: web_search]").length - 1,
    1,
    "one turn carries the tool request once",
  );

  // Queued and then composed again for the next turn: the draft is gone, so the
  // tool request cannot leak into a later turn.
  clearAttachments(key);
  assert.equal(
    composeTurnPrompt("Otra pregunta", {
      imageCount: 0,
      pdfs: [...pdfsFor(key)],
      intents: [...intentsFor(key)],
    }),
    "Otra pregunta",
  );
});

test("no PDF text ever touches localStorage", () => {
  const writes: string[] = [];
  const fake = {
    localStorage: {
      setItem: (key: string) => writes.push(key),
      getItem: (key: string) => {
        writes.push(key);
        return null;
      },
      removeItem: (key: string) => writes.push(key),
    },
  };
  const scope = globalThis as { window?: unknown };
  scope.window = fake;
  try {
    const key = "nabla.draft.pdf-storage";
    clearAttachments(key);
    addPdfAttachment(key, attachment("informe.pdf", ["Alpha"]));
    composeTurnPrompt("Resume esto", {
      imageCount: 0,
      pdfs: [...pdfsFor(key)],
      intents: [],
    });
    removePdfAttachment(key, pdfsFor(key)[0]!.id);
    clearAttachments(key);
  } finally {
    delete scope.window;
  }
  assert.deepEqual(writes, []);
});
