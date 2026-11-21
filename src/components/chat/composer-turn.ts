// What one composer turn really is: the typed question, the tool requests the
// human asked for, and the PDFs attached to it — composed in one place (S2-S3 of
// `odd/tasks/composer-tools-pdf.md`).
//
// Two functions, two audiences:
//
//   `composeTurnPrompt` builds the text the agent receives. It is the ONLY place
//   that turns attachments into prompt text, so a request cannot be delivered
//   twice and a document cannot be lost: the panel composes once per send or
//   queue, and the draft that produced it is cleared right after.
//
//   `composeTurnDisplay` builds the label a transcript row and a queue row keep.
//   It names the attachments (count, file names, requested tools) and never their
//   content: the extracted text stays out of the conversation history, exactly as
//   an image payload never enters the transcript.
//
// With nothing attached both answer the trimmed text unchanged, which is what
// keeps a text-only (and an image-only, through the store's own label) turn
// behaving exactly as it did before this feature existed.
import { pdfsPromptBlock, type ComposerPdf } from "./composer-pdfs";
import {
  toolIntentLabelKey,
  toolIntentRequestLines,
  type ComposerToolIntent,
} from "./composer-tools";
import { t } from "../../lib/i18n";

/** Everything one draft may add to a turn, besides the typed text. */
export interface TurnAttachments {
  /** How many images travel with the turn (their payload is separate). */
  imageCount: number;
  pdfs: readonly ComposerPdf[];
  intents: readonly ComposerToolIntent[];
}

/**
 * The prompt text of one turn: the question, then one line per requested tool,
 * then the page-numbered blocks of every attached PDF. Empty when the turn has
 * nothing to say — the caller decides whether that is a turn at all.
 */
export function composeTurnPrompt(
  text: string,
  attachments: TurnAttachments,
): string {
  const parts: string[] = [];
  const question = text.trim();
  if (question !== "") parts.push(question);
  const requests = toolIntentRequestLines(attachments.intents);
  if (requests.length > 0) parts.push(requests.join("\n"));
  const documents = pdfsPromptBlock(attachments.pdfs);
  if (documents !== "") parts.push(documents);
  return parts.join("\n\n");
}

/**
 * The label a transcript or queue row keeps: what was typed, then what travelled
 * with it. Localized, because a human reads it; the prompt above stays in the
 * language the agent is asked in.
 */
export function composeTurnDisplay(
  text: string,
  attachments: TurnAttachments,
): string {
  const question = text.trim();
  const lines = question === "" ? [] : [question];
  if (attachments.imageCount > 0) {
    lines.push(t("chat.message.images", { count: attachments.imageCount }));
  }
  if (attachments.pdfs.length > 0) {
    lines.push(
      t("chat.message.pdfs", {
        list: attachments.pdfs.map((pdf) => pdf.name).join(", "),
      }),
    );
  }
  if (attachments.intents.length > 0) {
    lines.push(
      t("chat.message.tools", {
        list: attachments.intents
          .map((intent) => t(toolIntentLabelKey(intent)))
          .join(", "),
      }),
    );
  }
  return lines.join("\n");
}
