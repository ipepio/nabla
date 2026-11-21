// Committing a text preference draft: the two web-search text fields of the
// settings pane are edited as drafts and written when they are committed
// (`odd/tasks/web-search.md`, S3).
//
// What a draft is worth at commit time is pure policy — trimmed, and `null` when
// the file already holds it — so it is exercised here through the module the pane
// calls. `preferences.ts` reaches the webview's shared state (`vue`) and the
// settings seam (`@/contracts/commands`); the alias is mapped here the same way the
// drawer tests map it, so the real module runs unchanged.
import { test } from "node:test";
import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import path from "node:path";
import { pathToFileURL } from "node:url";

const sourceRoot = pathToFileURL(
  path.resolve(import.meta.dirname, "..", "src") + path.sep,
).href;

registerHooks({
  resolve(specifier, context, nextResolve) {
    // `@/` is the app's alias for `src/`, set in `vite.config.ts`; Node has no
    // tsconfig paths support of its own. The `.ts` extension is added for the same
    // reason: the bundler resolves extensionless imports, Node does not.
    if (specifier.startsWith("@/")) {
      return nextResolve(sourceRoot + specifier.slice(2) + ".ts", context);
    }
    return nextResolve(specifier, context);
  },
});

const { preferenceDraftCommit } = await import("../src/lib/preferences.ts");

// S3: a draft that is already what the file holds writes nothing. Surrounding
// spaces are the exception: they are a typo nobody can see in the field, so the
// commit reports the trimmed value instead of rewriting `settings.json` with a
// string that differs only in whitespace.
test("a draft worth writing is the trimmed value, and an unchanged one is null", () => {
  assert.equal(preferenceDraftCommit("https://searx.example", "https://searx.example"), null);
  assert.equal(preferenceDraftCommit("  https://searx.example  ", "https://searx.example"), null);
  assert.equal(preferenceDraftCommit("https://other.example", "https://searx.example"), "https://other.example");
  assert.equal(preferenceDraftCommit("   ", ""), null);
  assert.equal(preferenceDraftCommit("  https://searx.example ", "  https://searx.example"), null);
});

// S3: clearing a field is a real choice, never "keep the old value" — the same
// rule the setters already document for the instance.
test("an emptied draft commits as an empty value", () => {
  assert.equal(preferenceDraftCommit("", "https://searx.example"), "");
  assert.equal(preferenceDraftCommit("   ", "https://searx.example"), "");
  assert.equal(preferenceDraftCommit("", ""), null);
});

// S3: only the ends are trimmed. A credential is data: `user:pass` keeps its
// colon, a token keeps its inner characters, and case is the human's.
test("only surrounding spaces are removed from a credential draft", () => {
  assert.equal(preferenceDraftCommit(" user:pass ", ""), "user:pass");
  assert.equal(preferenceDraftCommit("Tok en", ""), "Tok en");
  assert.equal(preferenceDraftCommit("TOKEN", "token"), "TOKEN");
});
