/**
 * nabla-permissions — the Nabla permission gate (odd/tasks/permission-modes.md).
 *
 * One `pi.on("tool_call")` handler that decides, per call, whether a tool may
 * run. The three modes and the tool families come from L3 of that task:
 *
 *   read   bash, powershell, edit, write  → blocked, with a reason
 *   write  edit, write                    → allowed
 *   write  bash, powershell               → asks the human (ctx.ui.confirm)
 *   yolo   everything                     → allowed, nothing is asked
 *   any    read, grep, find, ls           → always allowed (they only inspect)
 *   any    a tool this build does not name → allowed: v1 gates three families,
 *          not every tool that exists (the browser bridge and the MCP tools
 *          reach outside the workspace and are decided in their own feature)
 *
 * WHY THE MODE IS READ FROM A FILE, NOT FROM THE ENVIRONMENT (L4):
 * the child's environment is fixed at spawn time, so a mode baked into it would
 * only change when the session restarts — S2 forbids that. The app writes
 * `<data_dir>/permissions.json` (`{"mode":"write"}`) at boot and on every change,
 * forwards that absolute directory as `NABLA_DATA_DIR`, and this handler reads
 * the mode on EVERY call. A mode switch therefore affects the very next tool
 * call of a live session.
 *
 * FAIL-SAFE, ALWAYS TOWARDS `write`: an unreadable or malformed projection, a
 * missing `NABLA_DATA_DIR`, or an exception in this handler never degrade to
 * `yolo`. A file we cannot read reads as `write` (files allowed, commands asked),
 * and the handler's own `catch` blocks instead of allowing.
 *
 * SIBLING PREFLIGHT CAVEAT (verified fact, documented on purpose): in pi's
 * default parallel execution mode the tool calls of one assistant message are
 * preflighted sequentially and only then run concurrently. An `await
 * ctx.ui.confirm(...)` here holds that preflight, so the siblings of a command
 * waiting for an answer wait too. That is acceptable for this design — the human
 * is deciding about a command in a turn that is already blocked on them — but it
 * is why this file asks once per command and never chains questions.
 *
 * The host embeds this file at build time and writes it to
 * `<data_dir>/extensions/nabla-permissions.ts`, which is what `-e` loads at spawn
 * (next to `nabla-ask`). This file is the source of truth; the copy under the
 * data dir is regenerated. The decision table below mirrors `decide` in
 * `src-tauri/src/settings.rs` — same modes, same families, same wording for the
 * reason the model reads.
 */
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { readFileSync } from "node:fs";
import { join } from "node:path";

/** The word the projection file stores. */
type PermissionMode = "read" | "write" | "yolo";

/** L3: the tools that only inspect. Never gated, in any mode. */
const FREE_TOOLS = new Set(["read", "grep", "find", "ls"]);

/** L3: the tools that run a command. */
const SHELL_TOOLS = new Set(["bash", "powershell"]);

/** L3: the tools that write a file. */
const FILE_TOOLS = new Set(["edit", "write"]);

/**
 * S3: the veto the model reads in `read` mode. It names the mode, says nothing
 * was executed, and points at the chip — a model that only reads "blocked" tries
 * the same call again in a different shape.
 */
function readModeReason(tool: string): string {
  return (
    `Nabla is in "read" mode, so ${tool} was blocked and nothing was executed or ` +
    `written. Read mode only allows inspecting (read, grep, find, ls). The human ` +
    `set this mode with the permission chip in the chat header and can change it ` +
    `there; tell them what you need instead of retrying the same call.`
  );
}

/** S4: the veto for a command the human did not approve in `write` mode. */
function commandDeniedReason(tool: string): string {
  return (
    `The human did not approve that ${tool} command, so it was not run. Do not ` +
    `retry it as is: ask what to do differently, or propose a step that does not ` +
    `need the command.`
  );
}

/**
 * A command that had to be approveable but could not be: there is no UI to ask
 * in, so the honest answer is a veto, never a silent run.
 */
function noUiReason(tool: string): string {
  return (
    `Nabla is in "write" mode, so ${tool} needs the human's approval, and this ` +
    `session has no interface to ask them. The command was not run. Ask the human ` +
    `to run it themselves, or to switch the permission chip to "yolo" if they ` +
    `want commands to run unattended.`
  );
}

/**
 * L4: the current mode, read from the projection on every call.
 *
 * Fail-safe (L4): a missing `NABLA_DATA_DIR`, a file that is not there, a body
 * that does not parse, a `mode` that is not one of the three words, or any error
 * while reading all answer `"write"` — the middle ground. Reading failure must
 * never look like `"yolo"`.
 */
function currentMode(): PermissionMode {
  const dir = process.env.NABLA_DATA_DIR;
  if (typeof dir !== "string" || dir.trim() === "") return "write";
  try {
    const raw = readFileSync(join(dir, "permissions.json"), "utf8");
    const parsed = JSON.parse(raw) as { mode?: unknown };
    const mode = parsed?.mode;
    if (mode === "read" || mode === "write" || mode === "yolo") return mode;
    return "write";
  } catch {
    return "write";
  }
}

/**
 * The command of a shell tool call, verbatim, for the question and for the
 * denial text. `bash` input is `{ command, timeout? }` (pi's documented shape);
 * an input that does not look like that is serialized whole rather than shown as
 * an empty line, because the human is being asked to approve exactly this.
 */
function commandOf(input: unknown): string {
  if (input !== null && typeof input === "object") {
    const command = (input as { command?: unknown }).command;
    if (typeof command === "string" && command.trim() !== "") return command;
  }
  try {
    return JSON.stringify(input) ?? "(no command in this call)";
  } catch {
    return "(no command in this call)";
  }
}

export default function nablaPermissions(pi: ExtensionAPI) {
  pi.on("tool_call", async (event, ctx) => {
    try {
      const tool = String(event.toolName ?? "");

      // L3: inspection is free in every mode, so this branch never asks and
      // never reads the file.
      if (FREE_TOOLS.has(tool)) return undefined;

      const mode = currentMode();

      // S5: yolo is yolo. Nothing is asked, nothing is blocked, and the UI says
      // so plainly — no euphemism hides what this mode does.
      if (mode === "yolo") return undefined;

      if (mode === "read") {
        if (SHELL_TOOLS.has(tool) || FILE_TOOLS.has(tool)) {
          return { block: true, reason: readModeReason(tool) };
        }
        // A tool Nabla does not gate in v1: allowed, and no question invented.
        return undefined;
      }

      // write mode: files are allowed, commands are asked.
      if (SHELL_TOOLS.has(tool)) {
        const command = commandOf(event.input);
        if (!ctx.hasUI) {
          // S4/fail-safe: no way to ask the human is not consent to run.
          return { block: true, reason: noUiReason(tool) };
        }
        const approved = await ctx.ui.confirm(
          "Run this command?",
          `Nabla is in "write" mode, so this command needs your approval.\n\n` +
            `${command}\n\n` +
            `Confirm to run it; cancel to block it.`,
        );
        if (approved !== true) {
          // Note: `false` folds "no" and "cancelled" together, so the model is
          // told it was not approved and never that it was cancelled.
          return { block: true, reason: commandDeniedReason(tool) };
        }
        return undefined;
      }

      return undefined;
    } catch (err) {
      // pi blocks a tool whose `tool_call` handler throws, but relying on that
      // would make a bug here look like a permission decision. Say it out loud.
      const detail = err instanceof Error ? err.message : String(err);
      return {
        block: true,
        reason:
          `Nabla could not decide whether this call is allowed (${detail}), so it ` +
          `was not executed. Nothing ran.`,
      };
    }
  });
}
