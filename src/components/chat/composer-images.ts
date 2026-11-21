// The rules of the composer's image input (S1-S2 of
// `odd/tasks/composer-image-input.md`), kept out of the component so they can be
// pinned by `tests/composer-images.test.ts`.
//
// Three rules live here, and every surface reads them from this module instead
// of restating them:
//
//   1. What the composer accepts: pi's four inline formats (`image/png`,
//      `image/jpeg`, `image/gif`, `image/webp`) and nothing else. pi converts
//      other formats itself, but a silent conversion is exactly the behaviour
//      this feature avoids — the human attaches what will be sent.
//   2. How much one turn carries: `COMPOSER_IMAGE_LIMIT` images, each at most
//      `COMPOSER_IMAGE_MAX_BYTES` decoded bytes. Rust enforces the same bounds
//      (`src-tauri/src/image_input.rs`) because the webview is not the authority.
//   3. What a paste means: a clipboard with an image *and* text pastes its text;
//      only an image-only clipboard (a screenshot) becomes attachments.
//
// Nothing here persists anything. The base64 payload lives in the reactive store
// of `src/lib/composer-attachments.ts` for as long as the draft exists, and it
// never reaches `localStorage`, a log line or the database.
import type { PromptImage } from "@/contracts/types";

/**
 * How many images one turn can carry. pi accepts any number on the wire, so the
 * bound is this app's: four screenshots is a prompt, a folder is a mistake.
 */
export const COMPOSER_IMAGE_LIMIT = 4;

/** Largest decoded image the composer attaches, in bytes (10 MiB). */
export const COMPOSER_IMAGE_MAX_BYTES = 10 * 1024 * 1024;

/** The formats pi sends inline without converting (`ImageContent.mimeType`). */
export const SUPPORTED_IMAGE_MIME_TYPES = [
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
] as const;

/** One attached image as the composer holds it: the wire payload plus its label. */
export interface ComposerImage extends PromptImage {
  /** Stable id inside the draft, so a preview can be removed by identity. */
  id: string;
  /** File name (or clipboard label), shown as the preview's alt text. */
  name: string;
  /** Decoded size, measured once when the image was accepted. */
  byteLength: number;
}

/** One image the platform just produced, before the rules accept it. */
export interface ImageCandidate {
  name: string;
  mimeType: string;
  /** Base64 payload, no data-url prefix. */
  data: string;
}

/** Why an image was not attached; the caller turns this into a message. */
export type ImageRejection = "unsupported" | "too-large" | "too-many";

/** Accepted count and payload ceiling, overridable so tests stay small. */
export interface ImageLimits {
  count: number;
  bytes: number;
}

export const DEFAULT_IMAGE_LIMITS: ImageLimits = {
  count: COMPOSER_IMAGE_LIMIT,
  bytes: COMPOSER_IMAGE_MAX_BYTES,
};

/**
 * The mime type pi understands, or `null`. `image/jpg` is the same format as
 * `image/jpeg` and parameters (`; charset=…`) are not part of the type, so both
 * are normalized here instead of being rejected as typos.
 */
export function normalizeImageMimeType(raw: string): string | null {
  const base = raw.split(";")[0]?.trim().toLowerCase() ?? "";
  if (base === "image/jpg") return "image/jpeg";
  return (SUPPORTED_IMAGE_MIME_TYPES as readonly string[]).includes(base) ? base : null;
}

/** Whether the payload is shaped like base64 (the shape pi/base64 expects). */
export function isBase64Data(data: string): boolean {
  if (data === "" || data.length % 4 !== 0) return false;
  const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
  const body = padding === 0 ? data : data.slice(0, -padding);
  if (body === "") return false;
  for (const char of body) {
    const accepted =
      (char >= "A" && char <= "Z") ||
      (char >= "a" && char <= "z") ||
      (char >= "0" && char <= "9") ||
      char === "+" ||
      char === "/";
    if (!accepted) return false;
  }
  return true;
}

/**
 * Decoded size of a base64 payload. `0` for an empty payload; callers pair this
 * with `isBase64Data`, which owns the shape check.
 */
export function base64ByteLength(data: string): number {
  if (data === "") return 0;
  const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
  return Math.max(0, (data.length / 4) * 3 - padding);
}

/**
 * The wire image a `FileReader` data url carries, or `null` when it is not one of
 * pi's formats. Used by the paste and picker paths, which both read a `Blob`.
 */
export function parseImageDataUrl(url: string): PromptImage | null {
  const match = /^data:([^;,]+);base64,(.*)$/s.exec(url);
  if (match === null) return null;
  const mimeType = normalizeImageMimeType(match[1] ?? "");
  if (mimeType === null) return null;
  const data = match[2] ?? "";
  if (!isBase64Data(data)) return null;
  return { mimeType, data };
}

let attachmentSequence = 0;

function nextComposerImageId(): string {
  attachmentSequence += 1;
  return `image-${attachmentSequence}`;
}

/**
 * Applies the rules to one candidate. A rejection answers the list exactly as it
 * was given, so a caller that ignores the reason can never half-attach an image.
 */
export function addComposerImage(
  list: readonly ComposerImage[],
  candidate: ImageCandidate,
  limits: ImageLimits = DEFAULT_IMAGE_LIMITS,
): { images: ComposerImage[]; rejection: ImageRejection | null } {
  const mimeType = normalizeImageMimeType(candidate.mimeType);
  if (mimeType === null || !isBase64Data(candidate.data)) {
    return { images: [...list], rejection: "unsupported" };
  }
  const byteLength = base64ByteLength(candidate.data);
  if (byteLength === 0) return { images: [...list], rejection: "unsupported" };
  if (byteLength > limits.bytes) return { images: [...list], rejection: "too-large" };
  if (list.length >= limits.count) return { images: [...list], rejection: "too-many" };
  const name = candidate.name.trim() === "" ? mimeType : candidate.name.trim();
  return {
    images: [
      ...list,
      { id: nextComposerImageId(), name, mimeType, data: candidate.data, byteLength },
    ],
    rejection: null,
  };
}

/** The list without one image; an unknown id is a no-op. */
export function removeComposerImage(
  list: readonly ComposerImage[],
  id: string,
): ComposerImage[] {
  return list.filter((image) => image.id !== id);
}

/**
 * What crosses the bridge: pi's `ImageContent`, with the preview-only fields
 * (id, name, size) dropped. The payload is never re-encoded, so what the human
 * sees is what the model receives.
 */
export function imagesToPromptImages(list: readonly ComposerImage[]): PromptImage[] {
  return list.map((image) => ({ mimeType: image.mimeType, data: image.data }));
}

/** One clipboard item, as `DataTransferItem` exposes it to a paste handler. */
export interface ClipboardItemLike {
  kind: string;
  type: string;
}

/**
 * Whether a paste is an image paste. A clipboard carrying both text and an image
 * pastes its text: pasting words must keep behaving like pasting words, and only
 * a screenshot (an image with no text) becomes attachments.
 */
export function shouldAttachFromClipboard(clipboard: {
  items: ArrayLike<ClipboardItemLike> | null | undefined;
  text: string;
}): boolean {
  if (clipboard.text.trim() !== "") return false;
  const items = clipboard.items;
  if (items === null || items === undefined) return false;
  for (let index = 0; index < items.length; index += 1) {
    const item = items[index];
    if (item !== undefined && item.kind === "file" && isImageType(item.type)) return true;
  }
  return false;
}

function isImageType(type: string): boolean {
  return normalizeImageMimeType(type) !== null;
}

/**
 * The transcript/queue label of a prompt that carries images: appended under the
 * typed text, or standing alone when the turn is image-only. Nothing about the
 * image is persisted — the label is the only thing a transcript keeps.
 */
export function appendImageLabel(text: string, label: string): string {
  const trimmed = text.trim();
  return trimmed === "" ? label : `${trimmed}\n${label}`;
}

/** One point and one rectangle, both already in the same unit. */
export interface Point {
  x: number;
  y: number;
}

export interface Rect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/**
 * Whether a native drop landed on the composer. Tauri reports the drop position
 * in physical pixels, while `getBoundingClientRect()` is in CSS pixels, so the
 * scale factor is part of the question.
 */
export function isInsideDropTarget(
  point: Point,
  rect: Rect,
  scale: number,
): boolean {
  const divisor = scale > 0 ? scale : 1;
  const x = point.x / divisor;
  const y = point.y / divisor;
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}
