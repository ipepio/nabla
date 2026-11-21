// Pending attachments and per-turn tool requests of the composer, one list per
// draft key (S1-S3 of `odd/tasks/composer-image-input.md` and
// `odd/tasks/composer-tools-pdf.md`).
//
// Why a module instead of refs in `ChatPanel.vue`: an attachment belongs to the
// draft it was attached under, exactly like the typed text
// (`use-prompt-draft.ts`), so the same key answers all of them —
// `nabla.draft.<sessionId>` or `nabla.draft.new.<workspaceId>` before the first
// send. Switching conversations therefore switches images, PDFs and tool requests
// too: something attached in one session is never offered in another (S2's
// isolation), and going back to the first one still has it.
//
// The payloads live in memory for as long as the draft does and **never** reach
// `localStorage`: a screenshot is not a preference, a PDF's extracted text is not
// one either, and a synchronous multi-megabyte write per attachment would also
// stall the webview. The preview is built from the same payload (`data:` url), so
// there is one copy, not two.
import { reactive } from "vue";
import {
  DEFAULT_IMAGE_LIMITS,
  addComposerImage,
  imagesToPromptImages,
  removeComposerImage,
  type ComposerImage,
  type ImageCandidate,
  type ImageLimits,
  type ImageRejection,
} from "../components/chat/composer-images";
import {
  DEFAULT_PDF_LIMITS,
  addComposerPdf,
  removeComposerPdf,
  type ComposerPdf,
  type PdfLimits,
  type PdfRejection,
} from "../components/chat/composer-pdfs";
import { toggleToolIntent, type ComposerToolIntent } from "../components/chat/composer-tools";
import type { PdfAttachment, PromptImage } from "../contracts/types";

/** One shared list per draft key; absent means "nothing attached yet". */
const pending = reactive(new Map<string, ComposerImage[]>());
const pendingPdfs = reactive(new Map<string, ComposerPdf[]>());
/** The tool requests chosen for one draft, in canonical order. */
const pendingIntents = reactive(new Map<string, ComposerToolIntent[]>());

/** Stable empty answers: the composer mounts with nothing attached very often. */
const NONE: readonly ComposerImage[] = Object.freeze([]);
const NO_PDFS: readonly ComposerPdf[] = Object.freeze([]);
const NO_INTENTS: readonly ComposerToolIntent[] = Object.freeze([]);

/** The images of one draft, oldest first. A missing key has none. */
export function attachmentsFor(key: string | null): readonly ComposerImage[] {
  if (key === null) return NONE;
  return pending.get(key) ?? NONE;
}

/** The PDFs of one draft, in attachment order. A missing key has none. */
export function pdfsFor(key: string | null): readonly ComposerPdf[] {
  if (key === null) return NO_PDFS;
  return pendingPdfs.get(key) ?? NO_PDFS;
}

/** The tool requests of one draft, in canonical order. */
export function intentsFor(key: string | null): readonly ComposerToolIntent[] {
  if (key === null) return NO_INTENTS;
  return pendingIntents.get(key) ?? NO_INTENTS;
}

/**
 * Applies one or more image candidates to a draft, in order, stopping at the first
 * rejection: a batch that would overflow keeps what already fit and reports why, so
 * the human is never told "4 added" when one was dropped. The rejection also
 * leaves everything already attached untouched.
 */
export function addAttachments(
  key: string | null,
  candidates: readonly ImageCandidate[],
  limits: ImageLimits = DEFAULT_IMAGE_LIMITS,
): { added: number; rejection: ImageRejection | null } {
  if (key === null) return { added: 0, rejection: null };
  let list = pending.get(key) ?? [];
  let added = 0;
  for (const candidate of candidates) {
    const result = addComposerImage(list, candidate, limits);
    if (result.rejection !== null) {
      if (added > 0) pending.set(key, result.images);
      return { added, rejection: result.rejection };
    }
    list = result.images;
    added += 1;
  }
  if (added > 0) pending.set(key, list);
  return { added, rejection: null };
}

/**
 * Applies one extraction to a draft. It is added at most once per call, reports
 * its own rejection (count, pages, characters, an empty extraction) and never
 * touches what is already attached.
 */
export function addPdfAttachment(
  key: string | null,
  candidate: PdfAttachment,
  limits: PdfLimits = DEFAULT_PDF_LIMITS,
): { added: number; rejection: PdfRejection | null } {
  if (key === null) return { added: 0, rejection: null };
  const list = pendingPdfs.get(key) ?? [];
  const result = addComposerPdf(list, candidate, limits);
  if (result.rejection !== null) return { added: 0, rejection: result.rejection };
  pendingPdfs.set(key, result.pdfs);
  return { added: 1, rejection: null };
}

/** Drops one image of a draft; an unknown key or id is a no-op. */
export function removeAttachment(key: string | null, id: string): void {
  if (key === null) return;
  const list = pending.get(key);
  if (list === undefined) return;
  const next = removeComposerImage(list, id);
  if (next.length === 0) pending.delete(key);
  else pending.set(key, next);
}

/** Drops one PDF of a draft; an unknown key or id is a no-op. */
export function removePdfAttachment(key: string | null, id: string): void {
  if (key === null) return;
  const list = pendingPdfs.get(key);
  if (list === undefined) return;
  const next = removeComposerPdf(list, id);
  if (next.length === 0) pendingPdfs.delete(key);
  else pendingPdfs.set(key, next);
}

/**
 * Adds or removes one tool request of a draft. This writes nothing else: no
 * setting changes, no tool runs, and the request is only text the next turn
 * carries (`composeTurnPrompt`).
 */
export function toggleIntent(key: string | null, intent: ComposerToolIntent): void {
  if (key === null) return;
  const next = toggleToolIntent(intentsFor(key), intent);
  if (next.length === 0) pendingIntents.delete(key);
  else pendingIntents.set(key, next);
}

/** Forgets everything attached to one draft: it was sent, queued, or emptied. */
export function clearAttachments(key: string | null): void {
  if (key === null) return;
  pending.delete(key);
  pendingPdfs.delete(key);
  pendingIntents.delete(key);
}

/**
 * Carries a draft's attachments and requests to another key.
 *
 * The first send creates the session (and therefore a new draft key) before pi
 * has accepted the prompt, so a *rejected* send would otherwise leave everything
 * behind the old key: the text stays on screen, the previews would vanish. A
 * failure moves them with the text, so nothing the human attached is lost.
 */
export function moveAttachments(from: string | null, to: string | null): void {
  if (from === null || to === null || from === to) return;
  moveMap(pending, from, to);
  moveMap(pendingPdfs, from, to);
  moveMap(pendingIntents, from, to);
}

/** One map's entry moved to `to` unless the target already owns its own. */
function moveMap<T>(map: Map<string, T[]>, from: string, to: string): void {
  const list = map.get(from);
  if (list === undefined) return;
  map.delete(from);
  if (map.has(to)) return;
  map.set(to, list);
}

/** What one draft's images become on the wire. */
export function attachmentsToPromptImages(
  key: string | null,
): PromptImage[] {
  return imagesToPromptImages(attachmentsFor(key));
}
