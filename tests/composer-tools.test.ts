// Behaviour of the composer's per-turn tool requests (S2 of
// `odd/tasks/composer-tools-pdf.md`).
//
// The rule this pins: a tool request is an explicit line of the turn's own text —
// delivered once, through the ordinary send or queue path — and never something
// the UI pretends is running. Selecting one writes no setting, executes nothing
// and leaves no state behind after the turn.
//
// The second half is availability, and it is deliberately conservative. A row is
// offered only when every local precondition holds, and `web-search` needs more
// than this build can prove: a switch in Settings and a bound MCP endpoint say
// nothing about a SearXNG instance answering a query, so the row stays visible,
// disabled, with the pending reason (S2: never advertise unverified SearXNG).
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { registerHooks } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  COMPOSER_TOOL_INTENTS,
  toggleToolIntent,
  toolIntentAvailable,
  toolIntentRequestLines,
  toolUnavailableReason,
  toolUnavailableReasonKey,
  type ToolUnavailableReason,
} from "../src/components/chat/composer-tools.ts";

const projectRoot = pathToFileURL(
  path.resolve(import.meta.dirname, "..") + path.sep,
).href;

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

const { composeTurnPrompt } = await import("../src/components/chat/composer-turn.ts");

const { clearAttachments, intentsFor, toggleIntent } = await import(
  "../src/lib/composer-attachments.ts"
);

const en = (await import("../src/locales/en.json", { with: { type: "json" } }))
  .default;
const es = (await import("../src/locales/es.json", { with: { type: "json" } }))
  .default;

/**
 * One message of a locale bundle, by its dotted i18n key. The catalogue is flat
 * (`src/lib/i18n.ts`), so the key is the property name itself.
 */
function message(bundle: Record<string, string>, key: string): string {
  const value = bundle[key];
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${key} is missing from this locale`);
  }
  return value;
}

/**
 * A run where every local fact this build can check holds: the search switch is
 * on, the MCP endpoint that publishes the tools is bound, and the human turned the
 * Browser tool on in Settings → Tools.
 */
const EVERYTHING_READY = {
  webSearchEnabled: true,
  toolEndpointReady: true,
  browserEnabled: true,
};

test("web search is not offered until the SearXNG round trip is proven end to end", () => {
  // The local evidence — switch on, endpoint bound — says nothing about a real
  // query and a real instance answering, so this build never advertises the
  // request (S2: do not advertise unverified SearXNG as available).
  assert.ok(
    COMPOSER_TOOL_INTENTS.includes("web-search"),
    "the row keeps its place in the menu, disabled",
  );
  assert.equal(toolIntentAvailable("web-search", EVERYTHING_READY), false);
  assert.equal(
    toolUnavailableReason("web-search", EVERYTHING_READY),
    "search-verification-pending",
  );
  // No combination of the local switches promotes the row: turning the
  // preference on, or having the endpoint bound, is not proof of anything.
  for (const webSearchEnabled of [true, false]) {
    for (const toolEndpointReady of [true, false]) {
      const context = { ...EVERYTHING_READY, webSearchEnabled, toolEndpointReady };
      assert.equal(toolIntentAvailable("web-search", context), false);
      assert.equal(
        toolUnavailableReason("web-search", context),
        "search-verification-pending",
      );
    }
  }
});

test("the browser request needs the Browser switch in Settings → Tools", () => {
  assert.equal(toolIntentAvailable("browser", EVERYTHING_READY), true);
  assert.equal(toolUnavailableReason("browser", EVERYTHING_READY), null);
  // S5 of `tool-drawer-and-mcp.md`: the switch ships off because the browser
  // drives a real Chromium instance, so the row says how to turn it on.
  const off = { ...EVERYTHING_READY, browserEnabled: false };
  assert.equal(toolIntentAvailable("browser", off), false);
  assert.equal(toolUnavailableReason("browser", off), "browser-off");
  // The web search switch has nothing to do with this row.
  assert.equal(
    toolIntentAvailable("browser", { ...EVERYTHING_READY, webSearchEnabled: false }),
    true,
  );
});

test("the local endpoint still gates every request", () => {
  // `web_search` and the browser tools are published from the endpoint this run
  // bound, so without it the agent has neither — whatever the switches say.
  const unbound = { ...EVERYTHING_READY, toolEndpointReady: false };
  assert.equal(toolIntentAvailable("browser", unbound), false);
  assert.equal(toolUnavailableReason("browser", unbound), "endpoint-unavailable");
  // The missing endpoint is the first thing to say, even with the switch off.
  assert.equal(
    toolUnavailableReason("browser", { ...unbound, browserEnabled: false }),
    "endpoint-unavailable",
  );
  // Search never gets that far: the unproven round trip is its whole answer.
  assert.equal(
    toolUnavailableReason("web-search", unbound),
    "search-verification-pending",
  );
  assert.deepEqual([...COMPOSER_TOOL_INTENTS], ["web-search", "browser"]);
});

test("every unavailable reason is a sentence, translated, in both locales", () => {
  const reasons: ToolUnavailableReason[] = [
    "search-verification-pending",
    "endpoint-unavailable",
    "web-search-off",
    "browser-off",
  ];
  for (const reason of reasons) {
    const key = toolUnavailableReasonKey(reason);
    // A missing translation throws, so this also pins that the key set is whole.
    const english = message(en, key);
    const spanish = message(es, key);
    assert.ok(english.trim().length > 10, `${key} is a sentence, not a label`);
    assert.ok(spanish.trim().length > 10, `${key} is a sentence, not a label`);
    assert.notEqual(spanish, english, `${key} is translated, not copied`);
  }
  // The pending reason names exactly what is missing, and promises nothing.
  assert.equal(
    message(en, toolUnavailableReasonKey("search-verification-pending")),
    "End-to-end search verification pending",
  );
});

test("toggling adds one intent once and removes it by identity", () => {
  const empty: readonly ("web-search" | "browser")[] = [];
  const withSearch = toggleToolIntent(empty, "web-search");
  assert.deepEqual(withSearch, ["web-search"]);
  assert.deepEqual(empty, [], "the original list is never mutated");
  // A second toggle of the same intent is the undo, not a duplicate.
  assert.deepEqual(toggleToolIntent(withSearch, "web-search"), []);
  // Two intents keep the canonical order regardless of click order.
  const both = toggleToolIntent(toggleToolIntent(empty, "browser"), "web-search");
  assert.deepEqual(both, ["web-search", "browser"]);
});

test("each request is one explicit line of the turn, and only once", () => {
  assert.deepEqual(toolIntentRequestLines([]), []);
  assert.deepEqual(toolIntentRequestLines(["web-search"]), [
    "[Requested tool: web_search] Use the web_search tool for this turn.",
  ]);
  assert.deepEqual(toolIntentRequestLines(["browser"]), [
    "[Requested tool: browser] Use the browser tools (open pages, read them, click) for this turn.",
  ]);
  const lines = toolIntentRequestLines(["browser", "web-search"]);
  assert.equal(lines.length, 2, "one line per request, never a duplicate");
  const prompt = composeTurnPrompt("Busca esto", {
    imageCount: 0,
    pdfs: [],
    intents: ["web-search"],
  });
  assert.equal(
    prompt,
    [
      "Busca esto",
      "",
      "[Requested tool: web_search] Use the web_search tool for this turn.",
    ].join("\n"),
  );
  assert.equal(
    prompt.split("[Requested tool: web_search]").length - 1,
    1,
    "one turn carries the request exactly once",
  );
});

test("requests belong to the draft they were chosen under, never to a setting", () => {
  const keyA = "nabla.draft.tool-a";
  const keyB = "nabla.draft.tool-b";
  clearAttachments(keyA);
  clearAttachments(keyB);
  assert.deepEqual(intentsFor(null), []);

  toggleIntent(keyA, "web-search");
  assert.deepEqual(intentsFor(keyA), ["web-search"]);
  assert.deepEqual(intentsFor(keyB), [], "one conversation's request never leaks into another");

  toggleIntent(keyA, "web-search");
  assert.deepEqual(intentsFor(keyA), []);

  // Queued or sent, the draft's requests are forgotten with it: the next turn
  // starts with no request the human did not make.
  toggleIntent(keyB, "browser");
  clearAttachments(keyB);
  assert.deepEqual(intentsFor(keyB), []);
});

test("choosing a request executes nothing and changes no preference", () => {
  // The rules are pure: the context is an input, and nothing about it is read,
  // written or remembered. Toggling twice leaves the world exactly as it was.
  const context = { ...EVERYTHING_READY, webSearchEnabled: false };
  const before = JSON.stringify(context);
  toggleToolIntent([], "web-search");
  toggleToolIntent(toggleToolIntent([], "web-search"), "web-search");
  assert.equal(toolIntentAvailable("web-search", context), false);
  assert.equal(
    toolUnavailableReason("web-search", context),
    "search-verification-pending",
  );
  assert.equal(JSON.stringify(context), before);
});
