// Pending images of the composer, one list per draft key (S1-S2 of
// `odd/tasks/composer-image-input.md`).
//
// Why a module instead of refs in `ChatPanel.vue`: an image belongs to the draft
// it was attached under, exactly like the typed text (`use-prompt-draft.ts`), so
// the same key answers both — `nabla.draft.<sessionId>` or
// `nabla.draft.new.<workspaceId>` before the first send. Switching conversations
// therefore switches images too: an attachment typed into one session is never
// offered in another (S2's isolation), and going back to the first one still has
// it.
//
// The payload is the base64 the model will receive. It lives in memory for as
// long as the draft does and **never** reaches `localStorage`: a screenshot is
// not a preference, and a synchronous 10 MiB write per image would also stall the
// webview. The preview is built from the same string (`data:` url), so there is
// one copy of the payload, not two.
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
import type { PromptImage } from "../contracts/types";

/** One shared list per draft key; absent means "no images attached yet". */
const pending = reactive(new Map<string, ComposerImage[]>());

/** Stable empty answer: the composer mounts with no images very often. */
const NONE: readonly ComposerImage[] = Object.freeze([]);

/** The images of one draft, oldest first. A missing key has none. */
export function attachmentsFor(key: string | null): readonly ComposerImage[] {
  if (key === null) return NONE;
  return pending.get(key) ?? NONE;
}

/**
 * Applies one or more candidates to a draft, in order, stopping at the first
 * rejection: a batch that would overflow keeps what already fit and reports why,
 * so the human is never told "4 added" when one was dropped. The rejection also
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

/** Drops one image of a draft; an unknown key or id is a no-op. */
export function removeAttachment(key: string | null, id: string): void {
  if (key === null) return;
  const list = pending.get(key);
  if (list === undefined) return;
  const next = removeComposerImage(list, id);
  if (next.length === 0) pending.delete(key);
  else pending.set(key, next);
}

/** Forgets the images of one draft: it was sent, queued, or emptied. */
export function clearAttachments(key: string | null): void {
  if (key === null) return;
  pending.delete(key);
}

/**
 * Carries a draft's images to another key.
 *
 * The first send creates the session (and therefore a new draft key) before pi
 * has accepted the prompt, so a *rejected* send would otherwise leave the images
 * behind the old key: the text stays on screen, the previews would vanish. A
 * failure moves them with the text, so nothing the human attached is lost.
 */
export function moveAttachments(from: string | null, to: string | null): void {
  if (from === null || to === null || from === to) return;
  const list = pending.get(from);
  if (list === undefined) return;
  pending.delete(from);
  if (pending.has(to)) return;
  pending.set(to, list);
}

/** What one draft's images become on the wire. */
export function attachmentsToPromptImages(
  key: string | null,
): PromptImage[] {
  return imagesToPromptImages(attachmentsFor(key));
}
