/**
 * nabla-subagents — the Nabla subagent helpers (odd/tasks/subagent-helpers.md, H2).
 *
 * Four tools that let a session delegate work to an autonomous child session:
 *
 *   run_agent          → create a child pi session, run one task, answer with its
 *                        final message (blocking: the parent waits, like any tool)
 *   agent_status       → this session's children and their state
 *   agent_cancel       → abort a running child
 *   agent_send_message → steer a running child with one more instruction
 *
 * The child is NOT a second process. `createAgentSession` (pi's own SDK, the
 * pattern H1 verified) builds an in-process agent with its own tool runner, so
 * the child costs no spawn, shares the parent's authentication, and writes a
 * real pi JSONL file that Nabla's existing transcript reader can render. That
 * file is what makes the "open the child like Herdr opens an agent" view cheap
 * (S1, S3): the host only reads a file.
 *
 * WHY THE ALLOWLIST IS THE PERMISSION GATE (H1.1): the parent's `tool_call` hook
 * never sees a child's tool calls — each `createAgentSession` builds a private
 * extension runtime. So the gate cannot be inherited at run time; it is applied
 * here, when the child's tool list is built: the profile's `tools` is
 * intersected with the human's permission mode, read from the same
 * `<NABLA_DATA_DIR>/permissions.json` projection `nabla-permissions.ts` reads, on
 * every run. In `read` mode the shell and file-writing tools are removed from
 * the list, and a tool that is not in the list does not exist for the child
 * (H1.2). The mode is read per child, so switching the chip affects the next
 * delegation without restarting anything.
 *
 * WHERE THE PROFILES COME FROM (S1 of `agent-profiles.md`): `<NABLA_DATA_DIR>/`
 * `agents.json` — the single file Settings → Subagents writes. Never a markdown
 * file inside the project: `.pi/agents/*.md` is pi's convention, and profiles are
 * Nabla's. The store is read on every tool call, so a profile the human enables,
 * disables or edits reaches the next delegation without a restart. Only profiles
 * with `enabled: true` are offered; when there is none, every tool answers that
 * honestly and points at Settings instead of failing (S3).
 *
 * WHETHER THIS EXTENSION EXISTS AT ALL is the host's decision, not ours (S3):
 * with the master switch off, or with no enabled profile, the spawn never passes
 * this file to `-e`, so `run_agent` does not exist for the model. A session that
 * is already running keeps what it started with — pi loads an extension once and
 * has no way to unload it — so the master switch is re-read by the host at the
 * next spawn and never here.
 *
 * NO RECURSION (S5): our own four tool names are removed from every child's list,
 * so a child can never delegate again. Children also start clean (H1.4): no
 * extensions are loaded, so there is nothing else to recurse into.
 *
 * NO QUESTIONS TO THE HUMAN (S6): a child is an autonomous worker. It has no ask
 * tools (they come from `nabla-ask`, an extension the child does not load) and
 * no question is ever forwarded; the parent asks through the `ask_*` bridge when a
 * decision is needed.
 *
 * THE HOST CONTRACT (H3) is a manifest, not a protocol:
 * `<NABLA_DATA_DIR>/subagents/<key>.json` is rewritten on every child start, end
 * and cancel, as an array of
 * `{ name, agent, sessionPath, status, startedAt, endedAt, parentSessionPath }`.
 * `parentSessionPath` is the parent's OWN JSONL file, learned from
 * `ctx.sessionManager.getSessionFile()` — it is the join key Rust needs to put a
 * child row under the chat that launched it (`sessions.parent_session_id`). The
 * file name is a hash of that path only so no long or unusual path has to become
 * a file name; the path itself always travels in the entries.
 *
 * The host embeds this file at build time and writes it to
 * `<data_dir>/extensions/nabla-subagents.ts`, which is what `-e` loads at spawn
 * (next to `nabla-ask` and the permission gate). This file is the source of
 * truth; the copy under the data dir is regenerated.
 */
import type {
  AgentSessionEvent,
  ExtensionAPI,
  ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import {
  CONFIG_DIR_NAME,
  DefaultResourceLoader,
  SessionManager,
  createAgentSession,
  getAgentDir,
} from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { createHash, randomUUID } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

/** The four tools this extension registers. Also the recursion strip list (S5). */
const OWN_TOOLS: readonly string[] = [
  "run_agent",
  "agent_status",
  "agent_cancel",
  "agent_send_message",
];

/**
 * S5: how many children one session may have running at the same time. The cap
 * is per parent session, not global: a child never delegates, so only the human's
 * chat can saturate it.
 */
const MAX_CHILDREN = 4;

/**
 * L3 of `permission-modes.md`: the tool families the gate knows by name. `read`
 * mode strips the shell and file-writing ones from a child's allowlist, the same
 * way the gate blocks them in the parent.
 */
const SHELL_TOOLS: readonly string[] = ["bash", "powershell"];
const FILE_TOOLS: readonly string[] = ["edit", "write"];

/**
 * One model this session can use, as the session's own registry holds it. Named
 * through the registry instead of importing pi-ai's `Model` here: the child is
 * built with exactly the object the parent's registry resolved, so a definition
 * cannot point the SDK at a model this process cannot authenticate.
 */
type SessionModel = ReturnType<ExtensionContext["modelRegistry"]["getAll"]>[number];

/** pi's own word for a thinking level; a definition's `effort` must be one. */
const EFFORTS: readonly string[] = [
  "off",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
];

/** The four states the host reads out of the manifest (H3.3). */
type ChildStatus = "queued" | "running" | "done" | "failed";

/**
 * One profile of `<NABLA_DATA_DIR>/agents.json`, as the Settings dialog stores it
 * (S1, S2 of `agent-profiles.md`). Mirrors `AgentProfile` in
 * `src-tauri/src/agents.rs` field by field; Rust validates before it writes, so
 * this is a reader only.
 */
interface AgentProfile {
  name: string;
  description: string;
  /** The child's allowlist, before the permission gate. Empty means no tools. */
  tools: string[];
  /** `provider/model` or a bare id, resolved against this session's models. */
  model?: string | null;
  /** A thinking level, when the profile asks for one. */
  effort?: string | null;
  /** The child's whole system prompt. */
  instructions: string;
  enabled: boolean;
}

/** The profile store, filtered down to what a delegation may use. */
interface ProfileStore {
  /** S6, second level of the model resolution order. */
  defaultModel?: string;
  /** Only the profiles with `enabled: true`, in file order. */
  profiles: AgentProfile[];
}

/** One child of this session, as the tools and the manifest report it. */
interface Child {
  name: string;
  agent: string;
  sessionPath: string;
  status: ChildStatus;
  /** ISO-8601, stamped when the child starts; `endedAt` when it settles. */
  startedAt: string | null;
  endedAt: string | null;
  /** The in-process session; absent once it settled (it is disposed then). */
  session?: Awaited<ReturnType<typeof createAgentSession>>["session"];
}

/** The parent's identity, as far as the host needs it (H2, the manifest key). */
interface ParentRef {
  /** The parent's own JSONL path, or `null` when this session has no file. */
  path: string | null;
  /** Stable key for the in-memory registry and the manifest file name. */
  key: string;
}

/** One manifest line. The host reads these; `parentSessionPath` is the join key. */
interface ManifestEntry {
  name: string;
  agent: string;
  sessionPath: string;
  status: ChildStatus;
  startedAt: string | null;
  endedAt: string | null;
  parentSessionPath: string | null;
}

// ── the profile store (S1, S3, S6 of `agent-profiles.md`) ────────────────────

/**
 * Reads `<NABLA_DATA_DIR>/agents.json` — the one file Settings → Subagents writes
 * — and answers the profiles this session may actually use.
 *
 * Read on every tool call, so an edit or a switch change reaches the next
 * delegation without restarting anything. A missing file, an unreadable one or a
 * body that does not parse is an empty store, never an exception: the tools then
 * answer honestly that there is nothing to delegate to (S3), which is what a
 * fresh install looks like.
 */
function loadProfiles(): ProfileStore {
  const dir = dataDir();
  if (!dir) return { profiles: [] };
  let raw: string;
  try {
    raw = readFileSync(join(dir, "agents.json"), "utf8");
  } catch {
    return { profiles: [] };
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return { profiles: [] };
  }
  if (typeof parsed !== "object" || parsed === null) return { profiles: [] };
  const config = parsed as {
    modelDefault?: unknown;
    profiles?: unknown;
  };
  const entries: unknown[] = Array.isArray(config.profiles) ? config.profiles : [];
  const profiles: AgentProfile[] = [];
  for (const entry of entries) {
    const profile = normalizeProfile(entry);
    // S3: a profile nobody enabled is not offered, and the host already decided
    // whether this extension exists at all.
    if (profile && profile.enabled) profiles.push(profile);
  }
  const defaultModel =
    typeof config.modelDefault === "string" && config.modelDefault.trim() !== ""
      ? config.modelDefault.trim()
      : undefined;
  return { defaultModel, profiles };
}

/**
 * One entry of the store, or `undefined` when it is not a profile at all (not an
 * object, no usable name). A field of the wrong type is dropped instead of
 * coerced: a profile is configuration, and guessing it would run a child with
 * settings the human never wrote.
 */
function normalizeProfile(entry: unknown): AgentProfile | undefined {
  if (typeof entry !== "object" || entry === null) return undefined;
  const raw = entry as Record<string, unknown>;
  const name = typeof raw.name === "string" ? raw.name.trim() : "";
  if (name === "") return undefined;
  const tools = Array.isArray(raw.tools)
    ? raw.tools
        .filter((tool): tool is string => typeof tool === "string")
        .map((tool) => tool.trim())
        .filter((tool) => tool !== "")
    : [];
  return {
    name,
    description: typeof raw.description === "string" ? raw.description.trim() : "",
    tools,
    model: typeof raw.model === "string" ? raw.model.trim() : undefined,
    effort: typeof raw.effort === "string" ? raw.effort.trim().toLowerCase() : undefined,
    instructions: typeof raw.instructions === "string" ? raw.instructions.trim() : "",
    enabled: raw.enabled === true,
  };
}

/**
 * The list `run_agent` prints when the name does not match (like
 * `resolve_view`): the profiles the human enabled, with what each is for.
 */
function describeProfiles(profiles: AgentProfile[]): string {
  return profiles
    .map((profile) =>
      profile.description
        ? `${profile.name} (${profile.description})`
        : profile.name,
    )
    .join(", ");
}

/**
 * S3: what every tool answers when nothing is enabled. It names the place the
 * human fixes it instead of failing silently or pretending the feature exists.
 */
function noProfilesText(): string {
  return (
    "No subagent profile is enabled, so there is nothing to delegate to. Turn on " +
    "the profiles you want in Settings → Subagents (and the master switch, if it " +
    "is off), then try again."
  );
}

// ── the permission gate, applied to the allowlist (H1.1, H1.2, S5) ──────────

/**
 * The human's current mode, read from the same projection the gate reads on every
 * tool call. Fail-safe towards `write`, exactly like `nabla-permissions.ts`: a
 * missing dir, a missing file, a body that does not parse or a word this build
 * does not know must never read as `yolo`.
 */
function currentMode(): "read" | "write" | "yolo" {
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
 * The child's real allowlist: the profile's tools, minus our own four (no
 * recursion), minus the shell and file-writing families when the human is in
 * `read` mode. An empty result is refused by the caller — a profile that lists no
 * tool (or only ones the mode blocks) would give the child nothing to work with,
 * so spawning it would be a lie.
 */
function childAllowlist(definition: AgentProfile): string[] {
  const blocked = currentMode() === "read" ? [...SHELL_TOOLS, ...FILE_TOOLS] : [];
  return definition.tools.filter(
    (tool) => !OWN_TOOLS.includes(tool) && !blocked.includes(tool),
  );
}

// ── the child session (H1.3) ────────────────────────────────────────────────

/** `<NABLA_DATA_DIR>`, or `undefined` when the host did not provide one. */
function dataDir(): string | undefined {
  const dir = process.env.NABLA_DATA_DIR;
  return typeof dir === "string" && dir.trim() !== "" ? dir : undefined;
}

/**
 * Where a child writes its JSONL.
 *
 * Under the data dir, as H1.3 decided: deterministic, outside the workspace, and
 * next to the manifests the host reads. A session started without a data dir
 * (the extension loaded by hand in a terminal) still gets somewhere durable —
 * `<cwd>/.pi/subagent-sessions` — and simply publishes no manifest, because the
 * host it would publish to is not running.
 */
function childSessionDir(cwd: string): string {
  const dir = dataDir();
  if (dir) return join(dir, "sessions", "subagents");
  return join(cwd, CONFIG_DIR_NAME, "subagent-sessions");
}

/**
 * The parent's identity for the registry and the manifest.
 *
 * `ctx.sessionManager.getSessionFile()` is the parent's own JSONL path — the one
 * field the host needs, and it is available in every tool context (H2). A session
 * with no file (an in-memory one) is not linked to anything: children still run,
 * keyed by the workspace, and the manifest they publish carries
 * `parentSessionPath: null`, which the host skips on purpose instead of guessing.
 */
function parentRefOf(ctx: { sessionManager: { getSessionFile(): string | undefined }; cwd: string }): ParentRef {
  const path = ctx.sessionManager.getSessionFile() ?? null;
  if (path) return { path, key: path };
  return { path: null, key: `workspace:${ctx.cwd}` };
}

/**
 * Rewrites the manifest of one parent (H3, the host contract).
 *
 * Synchronous on purpose: it is called from `session_shutdown` too, where an
 * async write would race the process going away. A failure is remembered and
 * reported in the next tool result instead of thrown — losing the manifest
 * degrades the host's view of the children, it does not break their run.
 */
function publishManifest(
  parent: ParentRef,
  children: Child[],
): string | undefined {
  const dir = dataDir();
  if (!dir) return undefined;
  try {
    const target = join(dir, "subagents");
    mkdirSync(target, { recursive: true });
    const digest = createHash("sha256").update(parent.key).digest("hex");
    const file = join(target, `${digest.slice(0, 32)}.json`);
    const entries: ManifestEntry[] = children.map((child) => ({
      name: child.name,
      agent: child.agent,
      sessionPath: child.sessionPath,
      status: child.status,
      startedAt: child.startedAt,
      endedAt: child.endedAt,
      parentSessionPath: parent.path,
    }));
    writeFileSync(file, `${JSON.stringify(entries, null, 2)}\n`, "utf8");
    return undefined;
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    return `the subagent manifest could not be written (${detail}), so Nabla's sidebar will not show this child`;
  }
}

/** One ISO-8601 stamp, the same shape the store and the app use. */
function nowIso(): string {
  return new Date().toISOString();
}

/**
 * The last assistant text of a finished child: what `run_agent` answers with.
 * Empty when the child never produced one, which the caller says out loud instead
 * of returning an empty result the model would read as success.
 */
function finalText(session: {
  messages: readonly unknown[];
}): { text: string; failed: boolean; reason?: string } {
  for (let index = session.messages.length - 1; index >= 0; index -= 1) {
    const message = session.messages[index] as {
      role?: string;
      content?: unknown;
      stopReason?: string;
      errorMessage?: string;
    };
    if (message?.role !== "assistant") continue;
    const failed = message.stopReason === "error";
    const parts: string[] = [];
    if (typeof message.content === "string") parts.push(message.content);
    else if (Array.isArray(message.content)) {
      for (const part of message.content) {
        const piece = part as { type?: string; text?: unknown };
        if (piece?.type === "text" && typeof piece.text === "string") {
          parts.push(piece.text);
        }
      }
    }
    const text = parts.join("\n").trim();
    if (failed) {
      return {
        text,
        failed: true,
        reason:
          message.errorMessage ??
          "the child's last response ended with an error and no message",
      };
    }
    // A response with no text (only tool calls) is not the child's answer: keep
    // walking back until an assistant turn actually said something.
    if (text !== "") return { text, failed: false };
  }
  return { text: "", failed: false };
}

// ── the extension ───────────────────────────────────────────────────────────

export default function nablaSubagents(pi: ExtensionAPI) {
  /**
   * This session's children, in start order. Per extension instance, which pi
   * scopes to one session: the registry is never shared across chats, and a
   * reload starts it empty (the manifest is what survives).
   */
  const children: Child[] = [];
  /** The last manifest failure, reported once in a tool result (see above). */
  let manifestError: string | undefined;
  // The parent's identity, learned from the first tool call. One copy keeps the
  // manifest key stable across every write of this instance, `session_shutdown`
  // included (a shutdown handler would otherwise have to guess it).
  let childrenParentPath: string | null = null;
  let childrenParentKey = "";

  const rememberParent = (parent: ParentRef) => {
    childrenParentPath = parent.path;
    childrenParentKey = parent.key;
  };

  const publish = (parent: ParentRef) => {
    const error = publishManifest(parent, children);
    if (error) manifestError = error;
  };

  const named = (name: string) =>
    children.find((child) => child.name === name) ??
    children.find((child) => child.name.toLowerCase() === name.toLowerCase());

  /** The children the human (and the host) still have to care about. */
  const running = () =>
    children.filter(
      (child) => child.status === "running" || child.status === "queued",
    );

  /**
   * A fresh name for a new child: the definition's name, suffixed when it is
   * taken (`review`, `review-2`) so one session can run the same agent twice
   * without `agent_cancel` becoming a guess.
   */
  const uniqueName = (base: string): string => {
    const clean = base.trim() === "" ? "agent" : base.trim();
    if (!named(clean)) return clean;
    for (let suffix = 2; ; suffix += 1) {
      const candidate = `${clean}-${suffix}`;
      if (!named(candidate)) return candidate;
    }
  };

  /** One line per child, the shape every status answer shares. */
  const describe = (child: Child): string =>
    `${child.name} [${child.agent}] ${child.status} — ${child.sessionPath}`;

  /** The trailing note a result carries when the manifest could not be written. */
  const manifestNote = (): string => {
    if (!manifestError) return "";
    const note = `\n\nNote: ${manifestError}.`;
    manifestError = undefined;
    return note;
  };

  /**
   * S6 has no exceptions, but a child that outlives its parent would: the run is
   * in-process, so a shutdown ends it anyway. Marking it here is what keeps the
   * host from showing a child as running forever after the chat was closed.
   */
  pi.on("session_shutdown", async () => {
    if (children.length === 0) return;
    const parent = { path: childrenParentPath, key: childrenParentKey };
    for (const child of running()) {
      child.status = "failed";
      child.endedAt = child.endedAt ?? nowIso();
      try {
        void child.session?.abort().catch(() => undefined);
      } catch {
        // The process is going away: nothing here can be repaired, and a throw
        // in shutdown would hide the manifest write below.
      }
      child.session = undefined;
    }
    publish(parent);
  });

  pi.registerTool({
    name: "run_agent",
    label: "Run Agent",
    description:
      "Delegate one task to a specialized agent profile and wait for its answer. " +
      "Profiles are configured by the person in Nabla's Settings → Subagents " +
      "(name, description, tools, optional model and effort, instructions); only " +
      "the enabled ones exist. The child runs as a separate pi session in this " +
      "same process, with only the tools its profile lists and the human's " +
      "permission mode allows, and writes its own session file, so the human can " +
      "watch it in Nabla's sidebar. This tool BLOCKS until the child finishes, " +
      "then returns the child's final message. The child cannot delegate further " +
      "and cannot ask the human anything: what it needs must be in `task`. At " +
      "most 4 children may run at once. Call it several times in one turn to run " +
      "agents in parallel.",
    promptSnippet: "Delegate one task to a configured agent profile and wait for it",
    promptGuidelines: [
      "run_agent blocks until the child answers: give it one self-contained task, not a conversation.",
      "run_agent children cannot ask the human anything — put every decision they need in the task.",
    ],
    parameters: Type.Object({
      agent: Type.String({
        description:
          "The agent profile to run, by its name as configured in Settings → Subagents (e.g. `explorer`).",
      }),
      task: Type.String({
        description:
          "The whole task for the child: what to do, where, and what to answer with. It cannot ask follow-up questions.",
      }),
      name: Type.Optional(
        Type.String({
          description:
            "A name for this run, used by agent_status, agent_cancel and agent_send_message. Defaults to the profile name, suffixed when taken.",
        }),
      ),
    }),
    async execute(_toolCallId, params, signal, onUpdate, ctx) {
      const parent = parentRefOf(ctx);
      rememberParent(parent);
      // S1/S3: the profile store is Nabla's file, read fresh, and only its
      // enabled profiles count.
      const store = loadProfiles();
      if (store.profiles.length === 0) {
        return {
          content: [{ type: "text" as const, text: noProfilesText() }],
          details: { agent: params.agent, sessionPath: null, status: "failed" },
        };
      }
      const wanted = String(params.agent ?? "").trim().toLowerCase();
      const definition = store.profiles.find(
        (entry) => entry.name.toLowerCase() === wanted,
      );
      if (!definition) {
        return {
          content: [
            {
              type: "text" as const,
              text:
                `No agent profile named \`${params.agent}\`. Enabled profiles: ` +
                `${describeProfiles(store.profiles)}.`,
            },
          ],
          details: { agent: params.agent, sessionPath: null, status: "failed" },
        };
      }

      // S5: the cap is checked before anything is created, and synchronously, so
      // four parallel calls in one assistant message cannot overshoot it.
      if (running().length >= MAX_CHILDREN) {
        return {
          content: [
            {
              type: "text" as const,
              text:
                `This session already has ${MAX_CHILDREN} children running ` +
                `(${running().map(describe).join("; ")}). Wait for one to finish, ` +
                `or cancel one with agent_cancel before starting another.`,
            },
          ],
          details: { agent: definition.name, sessionPath: null, status: "failed" },
        };
      }

      const allowlist = childAllowlist(definition);
      if (allowlist.length === 0) {
        const mode = currentMode();
        return {
          content: [
            {
              type: "text" as const,
              text:
                `The profile \`${definition.name}\` would give the child no tools ` +
                `under the current "${mode}" permission mode` +
                (definition.tools.length === 0
                  ? " (it lists none)"
                  : ` (it lists ${definition.tools.join(", ")})`) +
                `, so the child was not started. Give it at least one tool the ` +
                `mode allows, in Settings → Subagents.`,
            },
          ],
          details: { agent: definition.name, sessionPath: null, status: "failed" },
        };
      }

      // S6: the model resolution order is the profile's own model, then the
      // store's default, then this session's model (`undefined` below).
      const askedModel = definition.model || store.defaultModel;
      const model = resolveModel(ctx, askedModel);
      if (askedModel && !model) {
        return {
          content: [
            {
              type: "text" as const,
              text:
                `The profile \`${definition.name}\` asks for model ` +
                `\`${askedModel}\`, which this session cannot reach. Fix that ` +
                `profile's model in Settings → Subagents (or leave it empty to ` +
                `use this session's model) and try again.`,
            },
          ],
          details: { agent: definition.name, sessionPath: null, status: "failed" },
        };
      }
      if (definition.effort && !EFFORTS.includes(definition.effort.toLowerCase())) {
        return {
          content: [
            {
              type: "text" as const,
              text:
                `The profile \`${definition.name}\` asks for effort ` +
                `\`${definition.effort}\`, which is not a thinking level pi has ` +
                `(${EFFORTS.join(", ")}). Fix that profile's effort in Settings → ` +
                `Subagents and try again.`,
            },
          ],
          details: { agent: definition.name, sessionPath: null, status: "failed" },
        };
      }

      const name = params.name ? uniqueName(params.name) : uniqueName(definition.name);
      const sessionDir = childSessionDir(ctx.cwd);
      const sessionPath = join(sessionDir, `${randomUUID()}.jsonl`);
      const child: Child = {
        name,
        agent: definition.name,
        sessionPath,
        status: "queued",
        startedAt: null,
        endedAt: null,
      };
      children.push(child);
      publish(parent);

      let session: Child["session"];
      try {
        // H1.3: the child's JSONL is ours to place, so the host can index it
        // before the child has written a single line.
        const sessionManager = SessionManager.open(sessionPath, sessionDir, ctx.cwd);
        // H1.4 + S4: a clean child whose system prompt is exactly the profile's
        // `instructions`. No extension of the parent's is inherited (a gate that
        // runs in the parent cannot run here — H1.1), no skill or template
        // surprises the profile's author.
        const resourceLoader = new DefaultResourceLoader({
          cwd: ctx.cwd,
          agentDir: getAgentDir(),
          noExtensions: true,
          noSkills: true,
          noPromptTemplates: true,
          noThemes: true,
          noContextFiles: true,
          systemPrompt:
            definition.instructions === "" ? undefined : definition.instructions,
        });
        await resourceLoader.reload();
        const created = await createAgentSession({
          cwd: ctx.cwd,
          model,
          thinkingLevel: definition.effort
            ? (definition.effort.toLowerCase() as
                | "off"
                | "minimal"
                | "low"
                | "medium"
                | "high"
                | "xhigh"
                | "max")
            : undefined,
          // H1.2: a tool that is not listed does not exist for the child. Our own
          // four are stripped from the list AND denied, so a profile that names
          // `run_agent` cannot recurse (S5).
          // S6 of `agent-profiles.md`: `model` is the resolved profile/default one,
          // and `undefined` means "this session's model".
          tools: allowlist,
          excludeTools: ["mcp__*", ...OWN_TOOLS],
          resourceLoader,
          sessionManager,
        });
        session = created.session;
        child.session = session;
        child.status = "running";
        child.startedAt = nowIso();
        publish(parent);

        // Brief progress, the shape Nabla's tool card already renders: the last
        // tool the child used, without copying its conversation anywhere.
        let lastTool = "";
        let lastUpdate = 0;
        const unsubscribe = session.subscribe((event: AgentSessionEvent) => {
          if (event.type !== "tool_execution_start") return;
          lastTool = event.toolName;
          const at = Date.now();
          if (at - lastUpdate < 250) return;
          lastUpdate = at;
          onUpdate?.({
            content: [
              { type: "text" as const, text: `${name} is running (last tool: ${lastTool})` },
            ],
            details: {
              agent: definition.name,
              name,
              sessionPath,
              status: "running",
              lastTool,
            },
          });
        });

        // The parent's tool call being aborted (the human pressed stop) must stop
        // the child too: an orphan child in-process would keep burning tokens.
        const onAbort = () => {
          void session?.abort().catch(() => undefined);
        };
        signal?.addEventListener("abort", onAbort, { once: true });
        try {
          // `expandPromptTemplates: false`: the task is data, never a command or
          // a template — a child must not be steerable by what the parent writes.
          await session.prompt(String(params.task ?? ""), {
            expandPromptTemplates: false,
          });
        } finally {
          signal?.removeEventListener("abort", onAbort);
          unsubscribe();
        }

        const outcome = finalText(session);
        child.status = outcome.failed ? "failed" : "done";
        child.endedAt = nowIso();
        publish(parent);

        if (outcome.failed) {
          return {
            content: [
              {
                type: "text" as const,
                text:
                  `${name} (${definition.name}) failed: ${outcome.reason}. Its ` +
                  `session file is ${sessionPath}${outcome.text ? `, and its last ` +
                    `words were:\n\n${outcome.text}` : ""}.${manifestNote()}`,
              },
            ],
            details: {
              agent: definition.name,
              name,
              sessionPath,
              status: child.status,
              error: outcome.reason,
            },
          };
        }
        if (outcome.text === "") {
          child.status = "failed";
          return {
            content: [
              {
                type: "text" as const,
                text:
                  `${name} (${definition.name}) finished without a final message, ` +
                  `so there is nothing to report. Its session file is ${sessionPath}; ` +
                  `read it to see what it did.${manifestNote()}`,
              },
            ],
            details: {
              agent: definition.name,
              name,
              sessionPath,
              status: child.status,
            },
          };
        }
        return {
          content: [
            {
              type: "text" as const,
              text: `${outcome.text}${manifestNote()}`,
            },
          ],
          details: {
            agent: definition.name,
            name,
            sessionPath,
            status: child.status,
          },
        };
      } catch (err) {
        // A failure to even build the child is the child's failure, reported as
        // one: the model must not read a spawn error as "the task is done".
        child.status = "failed";
        child.endedAt = nowIso();
        publish(parent);
        const detail = err instanceof Error ? err.message : String(err);
        return {
          content: [
            {
              type: "text" as const,
              text:
                `${name} (${definition.name}) could not run: ${detail}. Its ` +
                `session file is ${sessionPath}.${manifestNote()}`,
            },
          ],
          details: {
            agent: definition.name,
            name,
            sessionPath,
            status: child.status,
            error: detail,
          },
        };
      } finally {
        // Dispose only when the run is over: while it is captured by an awaited
        // `prompt`, `child.session` must stay addressable for agent_cancel.
        if (child.status !== "running" && child.status !== "queued") {
          try {
            child.session?.dispose();
          } catch {
            // Disposal is bookkeeping: a child that already settled stays settled.
          }
          child.session = undefined;
        }
      }
    },
  });

  pi.registerTool({
    name: "agent_status",
    label: "Agent Status",
    description:
      "List the children this session started: their name, the agent definition " +
      "they run, their state (queued, running, done, failed) and the pi session " +
      "file each one writes. The human can open that file in Nabla's sidebar; " +
      "this tool is how you know what is still running before starting or " +
      "cancelling anything.",
    promptSnippet: "List this session's subagents and their state",
    parameters: Type.Object({
      name: Type.Optional(
        Type.String({
          description: "One child by its name. Omit it to list them all.",
        }),
      ),
    }),
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      const parent = parentRefOf(ctx);
      rememberParent(parent);
      // S3: with nothing enabled there is no profile to list and no run to
      // describe, so the honest answer is the one that points at Settings.
      const store = loadProfiles();
      if (children.length === 0) {
        return {
          content: [
            {
              type: "text" as const,
              text:
                store.profiles.length === 0
                  ? noProfilesText()
                  : "This session has not started any subagent yet. Use run_agent " +
                    `with one of: ${describeProfiles(store.profiles)}.`,
            },
          ],
          details: { children: [] },
        };
      }
      const asked = params.name ? named(String(params.name)) : undefined;
      if (params.name && !asked) {
        return {
          content: [
            {
              type: "text" as const,
              text:
                `No child named \`${params.name}\` in this session. Current ` +
                `children: ${children.map((child) => child.name).join(", ")}.`,
            },
          ],
          details: { children: children.map(summary) },
        };
      }
      const shown = asked ? [asked] : children;
      return {
        content: [
          {
            type: "text" as const,
            text: shown.map(describe).join("\n") + manifestNote(),
          },
        ],
        details: { children: shown.map(summary) },
      };
    },
  });

  pi.registerTool({
    name: "agent_cancel",
    label: "Agent Cancel",
    description:
      "Stop a running subagent of this session. The child's run is aborted, it " +
      "counts as failed (it did not finish), and the tools it was using stop. Use " +
      "it when a child is going the wrong way; it cannot be resumed afterwards, so " +
      "start a new run_agent with a sharper task instead.",
    promptSnippet: "Stop a running subagent of this session",
    parameters: Type.Object({
      name: Type.String({
        description: "The child to stop, as agent_status reports it.",
      }),
    }),
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      const parent = parentRefOf(ctx);
      rememberParent(parent);
      const child = named(String(params.name ?? ""));
      if (!child) {
        return {
          content: [
            {
              type: "text" as const,
              text:
                `No child named \`${params.name}\` in this session. Current ` +
                `children: ${children.length === 0 ? "none" : children.map((entry) => entry.name).join(", ")}.`,
            },
          ],
          details: { name: params.name, status: "unknown" },
        };
      }
      if (child.status !== "running" && child.status !== "queued") {
        return {
          content: [
            {
              type: "text" as const,
              text:
                `${child.name} already finished with status \`${child.status}\`, so ` +
                `there was nothing to cancel. Its session file is ${child.sessionPath}.`,
            },
          ],
          details: {
            agent: child.agent,
            sessionPath: child.sessionPath,
            status: child.status,
          },
        };
      }
      // The status is stamped before the abort settles: a cancel is the human's
      // decision, and the host must not wait on a promise it cannot observe.
      child.status = "failed";
      child.endedAt = nowIso();
      publish(parent);
      try {
        await child.session?.abort();
      } catch {
        // An abort that throws means the run was already over; the status above
        // is still the honest answer.
      }
      return {
        content: [
          {
            type: "text" as const,
            text:
              `${child.name} was cancelled. Its partial session file is ` +
              `${child.sessionPath}.${manifestNote()}`,
          },
        ],
        details: {
          agent: child.agent,
          sessionPath: child.sessionPath,
          status: child.status,
        },
      };
    },
  });

  pi.registerTool({
    name: "agent_send_message",
    label: "Agent Send Message",
    description:
      "Send one more instruction to a child that is still running, without " +
      "restarting it. The message is delivered as steering after the child's " +
      "current tool calls finish, so the child keeps everything it has already " +
      "done. Use it to correct course or to add a detail you forgot; a child that " +
      "already finished cannot be steered — start a new run_agent instead.",
    promptSnippet: "Send one more instruction to a running subagent",
    parameters: Type.Object({
      name: Type.String({
        description: "The child to steer, as agent_status reports it.",
      }),
      message: Type.String({
        description:
          "The instruction to add. It reaches the child mid-run, after its current tool calls.",
      }),
    }),
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      const parent = parentRefOf(ctx);
      rememberParent(parent);
      const child = named(String(params.name ?? ""));
      if (!child) {
        return {
          content: [
            {
              type: "text" as const,
              text:
                `No child named \`${params.name}\` in this session. Current ` +
                `children: ${children.length === 0 ? "none" : children.map((entry) => entry.name).join(", ")}.`,
            },
          ],
          details: { name: params.name, status: "unknown" },
        };
      }
      if (child.status !== "running" || !child.session) {
        // Inventing a re-run here would silently start work the human never
        // approved, and the child's context is gone: say so and stop.
        return {
          content: [
            {
              type: "text" as const,
              text:
                `${child.name} is not running (status \`${child.status}\`), so it ` +
                `cannot be steered. Its session file is ${child.sessionPath}. Start ` +
                `a new run_agent if the work must continue.`,
            },
          ],
          details: {
            agent: child.agent,
            sessionPath: child.sessionPath,
            status: child.status,
          },
        };
      }
      try {
        await child.session.prompt(String(params.message ?? ""), {
          expandPromptTemplates: false,
          streamingBehavior: "steer",
        });
      } catch (err) {
        const detail = err instanceof Error ? err.message : String(err);
        return {
          content: [
            {
              type: "text" as const,
              text:
                `The message did not reach ${child.name} (${detail}). It may have ` +
                `finished between the status read and the delivery; check ` +
                `agent_status before sending it again.`,
            },
          ],
          details: {
            agent: child.agent,
            sessionPath: child.sessionPath,
            status: child.status,
            error: detail,
          },
        };
      }
      return {
        content: [
          {
            type: "text" as const,
            text:
              `The message was queued for ${child.name} and is delivered after its ` +
              `current tool calls finish. Its state is unchanged until then; ` +
              `agent_status still reports \`${child.status}\`.`,
          },
        ],
        details: {
          agent: child.agent,
          sessionPath: child.sessionPath,
          status: child.status,
        },
      };
    },
  });

  /** One child as a payload: what the tools' `details` carry. */
  function summary(child: Child) {
    return {
      name: child.name,
      agent: child.agent,
      sessionPath: child.sessionPath,
      status: child.status,
      startedAt: child.startedAt,
      endedAt: child.endedAt,
    };
  }

  /**
   * Resolves a definition's `model` (`provider/model`, or a bare id) against the
   * models this session can actually use. An unreachable model is refused by the
   * caller: silently running on another model would change what the definition
   * asked for.
   */
  function resolveModel(
    ctx: Pick<ExtensionContext, "modelRegistry">,
    wanted: string | undefined,
  ): SessionModel | undefined {
    if (!wanted) return undefined;
    const asked = wanted.trim();
    const slash = asked.indexOf("/");
    if (slash > 0) {
      const provider = asked.slice(0, slash).trim();
      const id = asked.slice(slash + 1).trim();
      if (provider === "" || id === "") return undefined;
      return ctx.modelRegistry.find(provider, id) ?? undefined;
    }
    // A bare id: the definition named a model, not a provider, so the first one
    // this session knows wins. Ambiguity is the author's to fix, not ours to hide.
    const bare = asked.toLowerCase();
    return (
      ctx.modelRegistry
        .getAll()
        .find((model) => model.id.toLowerCase() === bare) ?? undefined
    );
  }
}
