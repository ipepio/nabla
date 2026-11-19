/**
 * nabla-ask — the Nabla question bridge (odd/tasks/agent-question-bridge.md).
 *
 * Three tools that let the agent ask the human a structured question instead of
 * writing "pick A or B" in prose and assuming the answer:
 *
 *   ask_select  → ctx.ui.select(title, options)        → the chosen option
 *   ask_confirm → ctx.ui.confirm(title, message)       → true / false
 *   ask_input   → ctx.ui.input(title, placeholder)     → the typed text
 *
 * In `--mode rpc` each `ctx.ui.*` call becomes one `extension_ui_request` line on
 * the child's stdout, and the promise resolves when the host writes the matching
 * `extension_ui_response` line on stdin (pi `docs/rpc-extension-ui.md`). Nabla
 * renders the question as a card in the chat of the session that asked it and
 * answers with the command `answer_ui_request`, so the human's answer becomes the
 * tool's return value and the turn continues with it.
 *
 * Deliberately NO timeout (S4): a question waits for the human for as long as the
 * turn needs, like this harness itself does. Cancelling the card — or stopping the
 * session — is what closes it, and the tool then reports the cancellation instead
 * of inventing an answer.
 *
 * The host embeds this file at build time and writes it to
 * `<data_dir>/extensions/nabla-ask.ts`, which is what `-e` loads at spawn. This
 * file is the source of truth; the copy under the data dir is regenerated.
 */
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

/** What every result of this extension carries in `details`. */
interface AskDetails {
  /** The human's answer, or `null` when the question was never answered. */
  answer: string | null;
  cancelled: boolean;
}

/** One tool result: `answer` for the model, `details` for rendering/state. */
function askResult(text: string, answer: string | null, cancelled: boolean) {
  const details: AskDetails = { answer, cancelled };
  return { content: [{ type: "text" as const, text }], details };
}

/**
 * The one sentence a cancelled question returns. It has to be unmistakable: a
 * model that reads "cancelled" as an empty string would quietly answer for the
 * human, which is the failure this feature exists to prevent.
 */
const CANCELLED =
  "The human cancelled this question, so there is no answer. Do not invent one " +
  "and do not assume either option: continue without it, ask a different way, " +
  "or say plainly what you still need.";

/** No UI at all (a mode without the extension UI sub-protocol): say so, ask in prose. */
const NO_UI =
  "This session has no interface for asking the human, so the question was not " +
  "shown to anyone. Ask it in your reply as plain text instead and continue with " +
  "the part that does not depend on the answer.";

export default function nablaAsk(pi: ExtensionAPI) {
  pi.registerTool({
    name: "ask_select",
    label: "Ask Select",
    description:
      "Ask the human to pick one of a few named options. The person sees the " +
      "question as a card in the Nabla chat and answers it there; this tool " +
      "returns the option they picked. Use it when a real decision is needed and " +
      "the alternatives can be listed — not for open-ended questions (use " +
      "ask_input) and not for a yes/no (use ask_confirm). It waits for the human " +
      "with no timeout. If they cancel, this tool says so: never guess an answer.",
    promptSnippet: "Ask the human to choose one of a few named options",
    parameters: Type.Object({
      title: Type.String({
        description: "The question, shown as the card's title. Be specific.",
      }),
      options: Type.Array(Type.String(), {
        description:
          "The options the human chooses from, in the order they should be shown. At least two, each short enough to read at a glance.",
      }),
    }),
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      const options = params.options
        .map((option: string) => option.trim())
        .filter((option: string) => option !== "");
      if (options.length < 2) {
        // Not the human's decision: the model asked a bad question, so it is
        // told to ask it again rather than shown a card with one choice.
        return askResult(
          "ask_select needs at least two distinct options. Ask again with the " +
            "alternatives you want the human to choose between.",
          null,
          false,
        );
      }
      if (!ctx.hasUI) return askResult(NO_UI, null, false);
      if (signal?.aborted) return askResult(CANCELLED, null, true);
      // No timeout option on purpose (S4): the human sets the pace.
      const choice = await ctx.ui.select(params.title, options);
      if (typeof choice !== "string") return askResult(CANCELLED, null, true);
      return askResult(`The human chose: ${choice}`, choice, false);
    },
  });

  pi.registerTool({
    name: "ask_confirm",
    label: "Ask Confirm",
    description:
      "Ask the human a yes/no question. The person sees the question as a card " +
      "in the Nabla chat with its message and answers it there; this tool " +
      "returns true only when they confirmed. Use it before an irreversible or " +
      "risky step, or to check a plan they have not seen yet. It waits for the " +
      "human with no timeout. A denial and a cancellation both come back as " +
      "not-confirmed, so never treat silence as a yes.",
    promptSnippet: "Ask the human a yes/no question before a risky step",
    parameters: Type.Object({
      title: Type.String({
        description: "The question, shown as the card's title.",
      }),
      message: Type.String({
        description:
          "What the human is actually confirming: the action, its consequence, and anything irreversible about it.",
      }),
    }),
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      if (!ctx.hasUI) return askResult(NO_UI, null, false);
      if (signal?.aborted) return askResult(CANCELLED, null, true);
      const confirmed = await ctx.ui.confirm(params.title, params.message);
      if (confirmed !== true) {
        // The protocol folds "no" and "cancelled" into the same `false`, so the
        // answer must not claim which one happened.
        return askResult(
          "The human did not confirm: they answered no or cancelled the " +
            "question. Treat it as a no, do not proceed with what was asked, " +
            "and do not re-ask the same question immediately.",
          "not-confirmed",
          false,
        );
      }
      return askResult("The human confirmed.", "confirmed", false);
    },
  });

  pi.registerTool({
    name: "ask_input",
    label: "Ask Input",
    description:
      "Ask the human for a short piece of text: a name, a path, a number, a " +
      "sentence you cannot infer. The person sees the question as a card in the " +
      "Nabla chat with a text field and answers it there; this tool returns what " +
      "they typed. Use it when the missing information is free-form — to pick " +
      "between named alternatives use ask_select instead. It waits for the human " +
      "with no timeout. If they cancel, this tool says so: never invent a value.",
    promptSnippet: "Ask the human for a short piece of text",
    parameters: Type.Object({
      title: Type.String({
        description: "The question, shown as the card's title.",
      }),
      placeholder: Type.Optional(
        Type.String({
          description:
            "A hint shown inside the empty field, e.g. the shape of the expected answer.",
        }),
      ),
    }),
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      if (!ctx.hasUI) return askResult(NO_UI, null, false);
      if (signal?.aborted) return askResult(CANCELLED, null, true);
      const text = await ctx.ui.input(params.title, params.placeholder);
      if (typeof text !== "string") return askResult(CANCELLED, null, true);
      if (text.trim() === "") {
        return askResult(
          "The human sent an empty answer, so there is nothing to use. Ask again " +
            "with a sharper question, or continue without that detail.",
          "",
          false,
        );
      }
      return askResult(`The human answered: ${text}`, text, false);
    },
  });
}