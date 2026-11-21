// The rules of the composer's PDF input (S3 of
// `odd/tasks/composer-tools-pdf.md`), kept out of every component so they can be
// pinned by `tests/composer-pdfs.test.ts`.
//
// The three rules, and the one this module exists for:
//
//   1. What arrives is what Rust extracted — `pages`, in page order. Page N is
//      `pages[N - 1]`, and an empty page is an empty string: an empty page keeps
//      its number instead of shifting the ones after it.
//   2. How much one turn carries: `COMPOSER_PDF_LIMIT` documents. Rust bounds the
//      file, the page count and the extracted text before this side ever sees it
//      (`src-tauri/src/pdf_input.rs`); the same bounds are re-applied here so a
//      forged payload cannot reach a prompt.
//   3. A document with no selectable text is a *scan*, and nothing here invents
//      text for it. It attaches (the human can see which file it is and remove
//      it) and `hasUnreadablePdf` is what makes the composer refuse to send it.
//
// Nothing here persists anything: the extracted text lives in the reactive store
// of `src/lib/composer-attachments.ts` for as long as the draft exists, and it
// never reaches `localStorage`, a log line or the database.
import type { PdfAttachment } from "@/contracts/types";

/** How many PDFs one turn can carry. */
export const COMPOSER_PDF_LIMIT = 3;

/** Pages one PDF may have; mirrors `MAX_PDF_PAGES` in `pdf_input.rs`. */
export const COMPOSER_PDF_MAX_PAGES = 200;

/** Characters of extracted text one PDF may carry; mirrors `MAX_PDF_TEXT_CHARS`. */
export const COMPOSER_PDF_MAX_CHARS = 1_000_000;

/** One attached PDF as the composer holds it. */
export interface ComposerPdf extends PdfAttachment {
  /** Stable id inside the draft, so a chip can be removed by identity. */
  id: string;
}

/** Why a PDF was not attached; the caller turns this into a message. */
export type PdfRejection = "too-many" | "too-large" | "too-many-pages" | "empty";

/** Accepted count and extraction ceilings, overridable so tests stay small. */
export interface PdfLimits {
  count: number;
  pages: number;
  chars: number;
}

export const DEFAULT_PDF_LIMITS: PdfLimits = {
  count: COMPOSER_PDF_LIMIT,
  pages: COMPOSER_PDF_MAX_PAGES,
  chars: COMPOSER_PDF_MAX_CHARS,
};

let attachmentSequence = 0;

function nextComposerPdfId(): string {
  attachmentSequence += 1;
  return `pdf-${attachmentSequence}`;
}

/**
 * Applies the rules to one extraction. A rejection answers the list exactly as it
 * was given, so a caller that ignores the reason can never half-attach a document.
 *
 * `pageCount`, `charCount` and `hasText` are derived from `pages` rather than
 * trusted from the payload: what the prompt will carry is the text, so the label
 * and the scan notice have to agree with it.
 */
export function addComposerPdf(
  list: readonly ComposerPdf[],
  candidate: PdfAttachment,
  limits: PdfLimits = DEFAULT_PDF_LIMITS,
): { pdfs: ComposerPdf[]; rejection: PdfRejection | null } {
  const pages = Array.isArray(candidate.pages) ? [...candidate.pages] : [];
  if (pages.length === 0) return { pdfs: [...list], rejection: "empty" };
  if (pages.length > limits.pages) {
    return { pdfs: [...list], rejection: "too-many-pages" };
  }
  const charCount = pages.reduce((total, page) => total + page.length, 0);
  if (charCount > limits.chars) return { pdfs: [...list], rejection: "too-large" };
  if (list.length >= limits.count) return { pdfs: [...list], rejection: "too-many" };
  const name = candidate.name.trim() === "" ? "document.pdf" : candidate.name.trim();
  return {
    pdfs: [
      ...list,
      {
        id: nextComposerPdfId(),
        name,
        pageCount: pages.length,
        pages,
        charCount,
        hasText: pages.some((page) => page.trim() !== ""),
        byteLength: candidate.byteLength,
      },
    ],
    rejection: null,
  };
}

/** The list without one PDF; an unknown id is a no-op. */
export function removeComposerPdf(
  list: readonly ComposerPdf[],
  id: string,
): ComposerPdf[] {
  return list.filter((pdf) => pdf.id !== id);
}

/**
 * Whether a document carries no selectable text at all — a scan. The composer
 * refuses to send one and says why: Nabla runs no OCR, so pretending the pages
 * were read is the one thing this feature never does.
 */
export function hasUnreadablePdf(pdfs: readonly ComposerPdf[]): boolean {
  return pdfs.some((pdf) => !pdf.hasText);
}

/**
 * The block one PDF becomes inside the turn's text. Page markers carry the page's
 * own number, so a conclusion can be attributed to the page it appeared on, and an
 * empty page contributes its marker and nothing else.
 */
export function pdfPromptBlock(pdf: ComposerPdf): string {
  const lines = [`--- PDF: ${pdf.name} (pages: ${pdf.pages.length}) ---`];
  pdf.pages.forEach((page, index) => {
    lines.push(`[page ${index + 1}]`);
    if (page !== "") lines.push(page);
  });
  lines.push(`--- end PDF: ${pdf.name} ---`);
  return lines.join("\n");
}

/** The blocks of every attached PDF, in attachment order; `""` for none. */
export function pdfsPromptBlock(pdfs: readonly ComposerPdf[]): string {
  return pdfs.map(pdfPromptBlock).join("\n\n");
}
