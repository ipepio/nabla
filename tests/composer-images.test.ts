// Behaviour of the composer's image input (S1-S2 of
// `odd/tasks/composer-image-input.md`).
//
// Two modules own it and both are pure enough to prove without a window:
//
// - `src/components/chat/composer-images.ts` holds the rules — what counts as a
//   supported image, how big and how many a turn accepts, what a pasted
//   clipboard means, and how a prompt with images is labelled in the transcript.
// - `src/lib/composer-attachments.ts` holds the pending images *per draft key*,
//   in memory only. Session isolation (an image typed into one conversation is
//   not offered in another) and the "no base64 anywhere persistent" rule are the
//   contract this file pins.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { registerHooks } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  COMPOSER_IMAGE_LIMIT,
  COMPOSER_IMAGE_MAX_BYTES,
  addComposerImage,
  appendImageLabel,
  base64ByteLength,
  isBase64Data,
  isInsideDropTarget,
  imagesToPromptImages,
  normalizeImageMimeType,
  parseImageDataUrl,
  removeComposerImage,
  shouldAttachFromClipboard,
  type ComposerImage,
} from "../src/components/chat/composer-images.ts";

const projectRoot = pathToFileURL(
  path.resolve(import.meta.dirname, "..") + path.sep,
).href;

// The app resolves extensionless relative imports (a bundler rule) and the `@/`
// alias; Node resolves neither on its own, so both are mapped here exactly like
// `tests/drawer-startup.test.ts` does. No component is stubbed: these modules
// import only Vue's reactivity.
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier.startsWith("@/")) {
      return nextResolve(projectRoot + specifier.slice(2), context);
    }
    if (!path.extname(specifier) && specifier.startsWith(".")) {
      const candidate = new URL(specifier + ".ts", context.parentURL);
      if (existsSync(fileURLToPath(candidate))) {
        return nextResolve(candidate.href, context);
      }
    }
    return nextResolve(specifier, context);
  },
});

const {
  addAttachments,
  attachmentsFor,
  clearAttachments,
  moveAttachments,
  removeAttachment,
} = await import("../src/lib/composer-attachments.ts");

/** One accepted attachment, with the smallest payload that is still base64. */
function image(at: string, data = "AAAA"): ComposerImage {
  return {
    id: at,
    name: `${at}.png`,
    mimeType: "image/png",
    data,
    byteLength: base64ByteLength(data),
  };
}

/** Deterministic base64 of a payload of `bytes` decoded bytes, for boundaries. */
test("a mime type is normalized to the four formats pi can send", () => {
  assert.equal(normalizeImageMimeType("image/png"), "image/png");
  assert.equal(normalizeImageMimeType("IMAGE/JPEG"), "image/jpeg");
  assert.equal(normalizeImageMimeType("image/jpg"), "image/jpeg");
  assert.equal(normalizeImageMimeType("image/gif"), "image/gif");
  assert.equal(normalizeImageMimeType("image/webp"), "image/webp");
  assert.equal(normalizeImageMimeType("image/png; charset=binary"), "image/png");
  // Anything else is not an image pi accepts inline: it would be converted or
  // dropped by pi, which is exactly the silent behaviour this feature refuses.
  for (const rejected of ["", "text/plain", "image/svg+xml", "image/bmp", "application/pdf"]) {
    assert.equal(normalizeImageMimeType(rejected), null, `${rejected} is not accepted`);
  }
});

test("base64 is measured by the bytes it really carries", () => {
  assert.equal(base64ByteLength(""), 0);
  assert.equal(base64ByteLength("Zg=="), 1);
  assert.equal(base64ByteLength("Zm8="), 2);
  assert.equal(base64ByteLength("Zm9v"), 3);
  assert.equal(base64ByteLength("Zm9vYg=="), 4);
  assert.equal(isBase64Data("Zm9vYg=="), true);
  assert.equal(isBase64Data(""), false);
  assert.equal(isBase64Data("Zm9v\n"), false);
  assert.equal(isBase64Data("Zm9v Yg=="), false);
  assert.equal(isBase64Data("not base64!"), false);
  assert.equal(isBase64Data("Zg="), false, "a dangling quantum is not base64");
});

test("a data url becomes the wire image pi expects, or nothing", () => {
  assert.deepEqual(parseImageDataUrl("data:image/png;base64,Zm9v"), {
    mimeType: "image/png",
    data: "Zm9v",
  });
  assert.deepEqual(parseImageDataUrl("data:image/jpg;base64,Zm9v"), {
    mimeType: "image/jpeg",
    data: "Zm9v",
  });
  for (const rejected of [
    "data:text/plain;base64,Zm9v",
    "data:image/png,Zm9v",
    "data:image/png;base64,",
    "data:image/png;base64,not base64!",
    "Zm9v",
    "",
  ]) {
    assert.equal(parseImageDataUrl(rejected), null, `${rejected} is not an image`);
  }
});

test("an accepted image keeps its bytes verbatim and a rejected one changes nothing", () => {
  const first = addComposerImage([], {
    name: "shot.png",
    mimeType: "image/png",
    data: "Zm9v",
  });
  assert.equal(first.rejection, null);
  assert.equal(first.images.length, 1);
  assert.equal(first.images[0]?.data, "Zm9v");
  assert.equal(first.images[0]?.byteLength, 3);

  // An unsupported type, an empty payload and an oversized one are refused
  // without touching the list the human already attached to.
  const list = first.images;
  for (const candidate of [
    { name: "notes.txt", mimeType: "text/plain", data: "Zm9v" },
    { name: "empty.png", mimeType: "image/png", data: "" },
  ]) {
    const result = addComposerImage(list, candidate);
    assert.equal(result.rejection, "unsupported");
    assert.deepEqual(result.images, list);
  }
  assert.equal(COMPOSER_IMAGE_MAX_BYTES, 10 * 1024 * 1024);
  const oversized = addComposerImage(
    list,
    { name: "huge.png", mimeType: "image/png", data: "Zm9v" },
    { count: COMPOSER_IMAGE_LIMIT, bytes: 2 },
  );
  assert.equal(oversized.rejection, "too-large");
  assert.deepEqual(oversized.images, list);
});

test("a turn carries at most the bounded number of images", () => {
  let list: ComposerImage[] = [];
  for (let index = 0; index < COMPOSER_IMAGE_LIMIT; index += 1) {
    const added = addComposerImage(list, {
      name: `${index}.png`,
      mimeType: "image/png",
      data: "Zm9v",
    });
    assert.equal(added.rejection, null, `image ${index} is accepted`);
    list = added.images;
  }
  assert.equal(list.length, COMPOSER_IMAGE_LIMIT);
  const overflow = addComposerImage(list, {
    name: "one-too-many.png",
    mimeType: "image/png",
    data: "Zm9v",
  });
  assert.equal(overflow.rejection, "too-many");
  assert.deepEqual(overflow.images, list);
});

test("removing one image leaves the others exactly as they were", () => {
  const list = [image("a"), image("b"), image("c")];
  assert.deepEqual(
    removeComposerImage(list, "b").map((entry) => entry.id),
    ["a", "c"],
  );
  assert.deepEqual(
    removeComposerImage(list, "missing").map((entry) => entry.id),
    ["a", "b", "c"],
  );
});

test("what crosses the bridge is pi's own ImageContent, never a preview", () => {
  assert.deepEqual(imagesToPromptImages([image("a", "Zm9v"), image("b", "YmFy")]), [
    { mimeType: "image/png", data: "Zm9v" },
    { mimeType: "image/png", data: "YmFy" },
  ]);
});

test("pasting text keeps its normal behaviour; a screenshot attaches", () => {
  const imageItem = { kind: "file", type: "image/png" };
  assert.equal(
    shouldAttachFromClipboard({ items: [imageItem], text: "" }),
    true,
    "a screenshot paste has no text, so it is an image turn",
  );
  assert.equal(
    shouldAttachFromClipboard({ items: [imageItem], text: "copied words" }),
    false,
    "a clipboard with text pastes its text",
  );
  assert.equal(
    shouldAttachFromClipboard({
      items: [{ kind: "string", type: "text/plain" }],
      text: "words",
    }),
    false,
  );
  assert.equal(shouldAttachFromClipboard({ items: [], text: "" }), false);
});

test("a prompt with images is labelled without repeating what was typed", () => {
  assert.equal(appendImageLabel("", "Images: 2"), "Images: 2");
  assert.equal(appendImageLabel("look at this", "Images: 1"), "look at this\nImages: 1");
});

test("a drop only lands on the composer that is under the pointer", () => {
  const rect = { left: 100, top: 200, right: 400, bottom: 320 };
  assert.equal(isInsideDropTarget({ x: 150, y: 250 }, rect, 1), true);
  assert.equal(isInsideDropTarget({ x: 100, y: 200 }, rect, 1), true);
  assert.equal(isInsideDropTarget({ x: 401, y: 250 }, rect, 1), false);
  assert.equal(isInsideDropTarget({ x: 90, y: 250 }, rect, 1), false);
  // Physical pixels at a 2x scale: the same point in CSS pixels is half.
  assert.equal(isInsideDropTarget({ x: 300, y: 500 }, rect, 2), true);
  assert.equal(isInsideDropTarget({ x: 150, y: 250 }, rect, 2), false);
});

test("pending images belong to the draft key they were attached under", () => {
  const keyA = "nabla.draft.s1";
  const keyB = "nabla.draft.s2";
  clearAttachments(keyA);
  clearAttachments(keyB);

  assert.equal(addAttachments(keyA, [{ name: "a.png", mimeType: "image/png", data: "Zm9v" }]).added, 1);
  assert.equal(addAttachments(keyB, [{ name: "b.png", mimeType: "image/png", data: "YmFy" }]).added, 1);

  assert.deepEqual(attachmentsFor(keyA).map((entry) => entry.name), ["a.png"]);
  assert.deepEqual(attachmentsFor(keyB).map((entry) => entry.name), ["b.png"]);
  assert.deepEqual(attachmentsFor(null), []);

  // Removing from one conversation never touches the other, and clearing the
  // first leaves the second intact: that is the isolation S2 asks for.
  removeAttachment(keyB, attachmentsFor(keyA)[0]!.id);
  assert.equal(attachmentsFor(keyA).length, 1);
  assert.equal(attachmentsFor(keyB).length, 1);
  clearAttachments(keyA);
  assert.deepEqual(attachmentsFor(keyA), []);
  assert.equal(attachmentsFor(keyB).length, 1);

  // A batch fills up to the bound and reports the rest; once full, a new
  // candidate is refused without changing what is attached.
  const fill = addAttachments(
    keyB,
    Array.from({ length: COMPOSER_IMAGE_LIMIT }, (_, index) => ({
      name: `${index}.png`,
      mimeType: "image/png",
      data: "Zm9v",
    })),
  );
  assert.equal(fill.added, COMPOSER_IMAGE_LIMIT - 1);
  assert.equal(fill.rejection, "too-many");
  assert.equal(attachmentsFor(keyB).length, COMPOSER_IMAGE_LIMIT);
  const overflow = addAttachments(keyB, [
    { name: "one-too-many.png", mimeType: "image/png", data: "Zm9v" },
  ]);
  assert.equal(overflow.added, 0);
  assert.equal(overflow.rejection, "too-many");
  assert.equal(attachmentsFor(keyB).length, COMPOSER_IMAGE_LIMIT);

  clearAttachments(keyB);
});

test("the first send moves a workspace's pending images to its new session", () => {
  const workspaceKey = "nabla.draft.new.7";
  const sessionKey = "nabla.draft.42";
  clearAttachments(workspaceKey);
  clearAttachments(sessionKey);

  addAttachments(workspaceKey, [{ name: "shot.png", mimeType: "image/png", data: "Zm9v" }]);
  moveAttachments(workspaceKey, sessionKey);

  assert.deepEqual(attachmentsFor(workspaceKey), []);
  assert.deepEqual(attachmentsFor(sessionKey).map((entry) => entry.name), ["shot.png"]);
  clearAttachments(sessionKey);
});

test("pending images never touch localStorage", () => {
  // The composer's text is persisted in a draft; the image payload is not, on
  // purpose: a base64 screenshot does not belong in localStorage.
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
    clearAttachments("nabla.draft.images");
    addAttachments("nabla.draft.images", [
      { name: "shot.png", mimeType: "image/png", data: "Zm9v" },
    ]);
    removeAttachment("nabla.draft.images", attachmentsFor("nabla.draft.images")[0]!.id);
    clearAttachments("nabla.draft.images");
  } finally {
    delete scope.window;
  }
  assert.deepEqual(writes, []);
});
