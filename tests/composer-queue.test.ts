// Queue ownership of an attached-PDF turn (S3 of
// `odd/tasks/composer-tools-pdf.md`).
//
// The composer composes a turn once — question, requested tools, page-numbered
// PDFs (`composer-turn.ts`) — and hands the same text to `enqueuePrompt` when the
// agent is working. Three things must hold, and this file pins them:
//
//   1. the queue keeps the composed text, so the document really travels when the
//      queue drains;
//   2. the row above the composer and the eventual transcript show the label, not
//      the extracted text;
//   3. a prompt without attachments behaves exactly as it did before the feature.
//
// `src/lib/session-store.ts` is a real store with a command seam, so the seam is
// resolved to `tests/support/commands-stub.ts` (see that file): no Tauri, no pi.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { registerHooks } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  addComposerPdf,
  pdfsPromptBlock,
  type ComposerPdf,
} from "../src/components/chat/composer-pdfs.ts";

const projectRoot = pathToFileURL(
  path.resolve(import.meta.dirname, "..") + path.sep,
).href;
const commandsStub = new URL("./support/commands-stub.ts", import.meta.url).href;

/** Node refuses a JSON module import without the attribute; the bundler accepts it. */
function withJsonAttribute(resolved: import("node:module").ResolveFnOutput) {
  if (!resolved.url.endsWith(".json")) return resolved;
  return { ...resolved, importAttributes: { type: "json" } };
}

registerHooks({
  resolve(specifier, context, nextResolve) {
    // The store's only connection to Rust: replaced by the test double so the
    // queue's own behavior can be observed without a bridge. Checked first,
    // because the relative form would otherwise resolve to the real seam.
    if (/^(\.\.?\/)+contracts\/commands$/.test(specifier) || specifier === "@/contracts/commands") {
      return nextResolve(commandsStub, context);
    }
    // The bundle aliases `@/` to `src/` and drops the extension; both are applied
    // here exactly as Vite does.
    if (specifier.startsWith("@/")) {
      const mapped = new URL("src/" + specifier.slice(2), projectRoot);
      const target = existsSync(fileURLToPath(mapped)) ? mapped.href : `${mapped.href}.ts`;
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

const { composeTurnDisplay, composeTurnPrompt } = await import(
  "../src/components/chat/composer-turn.ts"
);
const { enqueuePrompt, queuedCount, queuedPrompts, removeQueuedPrompt, sessionBuffer } =
  await import("../src/lib/session-store.ts");

/** One extracted report, exactly the shape Rust answers. */
function report(): ComposerPdf {
  const result = addComposerPdf([], {
    name: "informe.pdf",
    pageCount: 2,
    pages: ["Alpha conclusion", ""],
    charCount: 16,
    hasText: true,
    byteLength: 1024,
  });
  assert.equal(result.rejection, null);
  return result.pdfs[0]!;
}

test("a queued turn keeps the composed document and its label", () => {
  const session = "queue-pdf-session";
  const attachments = {
    imageCount: 0,
    pdfs: [report()],
    intents: ["web-search" as const],
  };
  const wireText = composeTurnPrompt("Resume este informe", attachments);
  const display = composeTurnDisplay("Resume este informe", attachments);

  enqueuePrompt(session, wireText, 7, [], display);

  assert.equal(queuedCount(session), 1);
  assert.deepEqual(queuedPrompts(session), [display]);
  // The queue owns the whole turn, document included: what it drains is the text
  // with the page markers, not a reference to a draft that has been cleared.
  const queued = sessionBuffer(session)?.queue[0];
  assert.equal(queued?.text, wireText);
  assert.equal(
    queued?.text.endsWith(pdfsPromptBlock(attachments.pdfs)),
    true,
    "the drain sends the page-numbered document itself",
  );
  assert.equal(queued?.display, display);
  assert.equal(display.includes("Alpha conclusion"), false);

  // Withdrawing the queued turn removes it and its muted transcript entry.
  removeQueuedPrompt(session, 0);
  assert.equal(queuedCount(session), 0);
  // An index that has already drained is ignored instead of throwing.
  removeQueuedPrompt(session, 0);
});

// PRESERVE: a prompt with nothing attached is queued under its own text, which is
// what the queue did before PDFs and tool requests existed.
test("a plain queued prompt keeps its old label", () => {
  const session = "queue-plain-session";
  enqueuePrompt(session, "hola", 8, [], null);
  assert.deepEqual(queuedPrompts(session), ["hola"]);
  assert.equal(sessionBuffer(session)?.queue[0]?.text, "hola");
  removeQueuedPrompt(session, 0);
  assert.equal(queuedCount(session), 0);
});
