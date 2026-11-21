// The rules of the composer's per-turn tool requests (S2 of
// `odd/tasks/composer-tools-pdf.md`), kept out of the component so they can be
// pinned by `tests/composer-tools.test.ts`.
//
// A tool request is *text in the turn*: one explicit line the human asked for,
// sent (or queued) once through the ordinary prompt path. Nothing here executes a
// tool, flips a setting, bypasses a permission or keeps state: choosing a request
// only appends a line to the prompt that turn carries, and the draft forgets it
// when the turn leaves the composer.
//
// Availability is a separate, honest answer, and this build can only prove part
// of it. Both tools are published from the same place: the local MCP endpoint this
// run binds and registers with pi. If that endpoint is not bound, the agent has
// neither the browser tools nor `web_search` — whatever `settings.json` says — so
// no request is offered.
//
// Above that, each request has its own human switch: `web-search` needs
// `settings.webSearchEnabled`, `browser` needs the Browser tool in
// Settings → Tools (`enabledTools`). And `web-search` needs something no local
// fact can supply: proof that the SearXNG round trip works end to end. A switch and
// a bound endpoint say nothing about an instance answering a query, so the request
// stays visible and disabled with that pending reason instead of promising a tool
// nobody has run (S2: do not advertise unverified SearXNG as available). Nothing
// here makes a network call, flips a setting or disables the underlying service.

/** The two requests the composer can attach to a turn. */
export const COMPOSER_TOOL_INTENTS = ["web-search", "browser"] as const;

export type ComposerToolIntent = (typeof COMPOSER_TOOL_INTENTS)[number];

/**
 * S2: whether `web_search` has been proven end to end in this build — a real turn,
 * the local endpoint, the registered tool and a SearXNG instance answering.
 *
 * It has not. The only evidence available locally is "the human turned the switch
 * on and the endpoint that publishes the tool is bound", which is not that proof,
 * so the row stays disabled with the pending reason. Flipping this constant is
 * what claims the evidence exists; it must land together with the QA that produced
 * it, never ahead of it.
 */
const webSearchEndToEndVerified = false;

/** Why a request cannot be offered right now; a closed set the UI translates. */
export type ToolUnavailableReason =
  | "search-verification-pending"
  | "endpoint-unavailable"
  | "web-search-off"
  | "browser-off";

/** What this run really has, read by the panel and passed in. */
export interface ToolIntentContext {
  /** `settings.webSearchEnabled`: the human turned the `web_search` tool on. */
  webSearchEnabled: boolean;
  /**
   * This run bound its local MCP endpoint and registered it with pi. Both tools
   * (`web_search` and the browser's) are published from there, so without it the
   * agent has neither and nothing can be requested.
   */
  toolEndpointReady: boolean;
  /**
   * S5 of `tool-drawer-and-mcp.md`: the Browser tool is on in Settings → Tools
   * (`enabledTools`). It ships off because the browser drives a real Chromium
   * instance, so a request that assumes it would be advertising a tool the human
   * never turned on.
   */
  browserEnabled: boolean;
}

/**
 * Why one request cannot be offered, or `null` when it can. Pure: the context is
 * the whole answer, so a caller never has to guess and a test never needs a
 * window. Availability is derived from this, so the disabled row and its reason
 * can never disagree.
 *
 * The first unmet precondition is the answer, in the order each case reads: the
 * unproven round trip outranks every local fact about `web-search` (nothing the
 * human switches on can make it honest today), and the shared endpoint outranks a
 * missing switch (no setting provides a tool the run never published).
 */
export function toolUnavailableReason(
  intent: ComposerToolIntent,
  context: ToolIntentContext,
): ToolUnavailableReason | null {
  switch (intent) {
    case "web-search":
      if (!webSearchEndToEndVerified) return "search-verification-pending";
      if (!context.toolEndpointReady) return "endpoint-unavailable";
      return context.webSearchEnabled ? null : "web-search-off";
    case "browser":
      if (!context.toolEndpointReady) return "endpoint-unavailable";
      return context.browserEnabled ? null : "browser-off";
    default:
      // Unreachable for this build's closed union; a newer intent this build does
      // not know is never advertised as available.
      return "endpoint-unavailable";
  }
}

/** Whether one request can be offered. */
export function toolIntentAvailable(
  intent: ComposerToolIntent,
  context: ToolIntentContext,
): boolean {
  return toolUnavailableReason(intent, context) === null;
}

/**
 * Adds or removes one request. The result is rebuilt in the canonical order of
 * `COMPOSER_TOOL_INTENTS`, so the line order of a turn never depends on which chip
 * was clicked first, and the input list is never mutated.
 */
export function toggleToolIntent(
  list: readonly ComposerToolIntent[],
  intent: ComposerToolIntent,
): ComposerToolIntent[] {
  const selected = new Set(list);
  if (selected.has(intent)) selected.delete(intent);
  else selected.add(intent);
  return COMPOSER_TOOL_INTENTS.filter((candidate) => selected.has(candidate));
}

/**
 * The request lines a turn carries, one per selected request, in canonical order.
 * The wording is the explicit request the human made — an instruction to the agent,
 * not a claim that a tool already ran — and it is the only thing a selection adds
 * to the turn.
 */
export function toolIntentRequestLines(
  intents: readonly ComposerToolIntent[],
): string[] {
  const selected = new Set(intents);
  const lines: string[] = [];
  for (const intent of COMPOSER_TOOL_INTENTS) {
    if (!selected.has(intent)) continue;
    lines.push(
      intent === "web-search"
        ? "[Requested tool: web_search] Use the web_search tool for this turn."
        : "[Requested tool: browser] Use the browser tools (open pages, read them, click) for this turn.",
    );
  }
  return lines;
}

/** The i18n key of one request's label in the chip and the menu. */
export function toolIntentLabelKey(
  intent: ComposerToolIntent,
): "chat.composer.tool.webSearch" | "chat.composer.tool.browser" {
  return intent === "web-search"
    ? "chat.composer.tool.webSearch"
    : "chat.composer.tool.browser";
}

/**
 * The i18n key of the reason a request is unavailable. The message is a sentence
 * the human can act on ("turn it on in Settings", "the endpoint is missing") or, for
 * the search round trip, a statement that the proof is still pending — never a bare
 * "disabled".
 */
export function toolUnavailableReasonKey(
  reason: ToolUnavailableReason,
):
  | "chat.composer.tool.searchVerificationPending"
  | "chat.composer.tool.endpointUnavailable"
  | "chat.composer.tool.webSearchOff"
  | "chat.composer.tool.browserOff" {
  switch (reason) {
    case "search-verification-pending":
      return "chat.composer.tool.searchVerificationPending";
    case "endpoint-unavailable":
      return "chat.composer.tool.endpointUnavailable";
    case "web-search-off":
      return "chat.composer.tool.webSearchOff";
    case "browser-off":
      return "chat.composer.tool.browserOff";
  }
}
