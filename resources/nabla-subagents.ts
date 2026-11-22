/**
 * nabla-subagents — the Nabla subagent helpers (odd/tasks/subagent-helpers.md, H2).
 *
 * Five tools that let a session delegate work to an autonomous child session:
 *
 *   run_agent          → create a child pi session, run one task, answer with its
 *                        final message (blocking: the parent waits, like any tool)
 *   run_agents         → launch up to four children at once, wait for all of them
 *                        to settle, and answer with one summary per child (S5 of
 *                        `agent-profiles.md`: dual judgment costs one wait, not two)
 *   agent_status       → this session's children and their state
 *   agent_cancel       → abort a running child
 *   agent_send_message → steer a running child with one more instruction
 *
 * ONE SPAWN PATH (S5): `run_agent` and `run_agents` differ only in how many
 * children they ask for and how they word the answer. Both validate through
 * `prepareDelegation` and create through `spawnChild`, into the same per-session
 * registry, so `agent_status` and `agent_cancel` address a batch's children
 * exactly like a single run's, and a batch child is indistinguishable in the
 * manifest from a `run_agent` child.
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
 * AN EMPTY STORE IS THE BUILT-IN TEMPLATES (S1 of `agent-profiles-defaults.md`).
 * Master switch on and no profile stored means the five templates Settings offers
 * — explorer, researcher, worker, judge, judge-2 — arrive enabled, so turning the
 * switch on is already a usable configuration instead of four tools that can only
 * answer "nothing to run". The text is NOT copied into this file: the host writes
 * it to `<NABLA_DATA_DIR>/extensions/agent-profile-defaults.json`, from the same
 * asset the Rust mirror embeds, and this file reads it on every tool call. A store
 * with anything in it — even a list of profiles the human turned off — is
 * authoritative, and the defaults never reappear beside it.
 *
 * WHETHER THIS EXTENSION EXISTS AT ALL is the host's decision, not ours (S3):
 * with the master switch off, or with a non-empty list where nothing is enabled,
 * the spawn never passes this file to `-e`, so `run_agent` does not exist for the
 * model. A session that is already running keeps what it started with — pi loads
 * an extension once and has no way to unload it — so the master switch is re-read
 * by the host at the next spawn and never here.
 *
 * NO RECURSION (S5): our own five tool names — `run_agent`, `run_agents` and the
 * three helpers — are removed from every child's list, so a child can never
 * delegate again, neither one at a time nor in a batch. Children also start
 * clean (H1.4): no extensions are loaded, so there is nothing else to recurse
 * into.
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

/** The five tools this extension registers. Also the recursion strip list (S5). */
const OWN_TOOLS: readonly string[] = [
  "run_agent",
  "run_agents",
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
 * S5 of `agent-profiles.md`: how many profiles ONE `run_agents` call launches.
 * Kept apart from `MAX_CHILDREN` on purpose — the session cap is what the human's
 * chat can sustain, this one is what a single call is allowed to ask for — even
 * though both are four today.
 */
const MAX_BATCH = 4;

/**
 * How much of a child's final answer a batch summary quotes. Enough for the
 * parent (and the human's card) to recognise what the child found, never the
 * whole file: the session path is always there for the rest.
 */
const EXCERPT_CHARS = 400;

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
  /**
   * Only the profiles with `enabled: true`, in file order — or the built-in
   * templates when the store is empty and the master switch is on (S1 of
   * `agent-profiles-defaults.md`).
   */
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

/**
 * How one child ended, as the shared spawn path reports it. `status` is the
 * child's own state; `failure` says how a failed child failed, so each caller can
 * word it honestly: `spawn` the session could not even be built, `run` its last
 * turn ended in an error, `empty` it finished without saying anything.
 */
interface ChildResult {
  child: Child;
  status: "done" | "failed";
  failure?: "spawn" | "run" | "empty";
  /** The child's final message; empty when it produced none. */
  text: string;
  /** The technical reason of a failed child. */
  reason?: string;
}

/** Everything one spawn needs: what the caller already validated, nothing else. */
interface SpawnRequest {
  ctx: ExtensionContext;
  parent: ParentRef;
  definition: AgentProfile;
  model: SessionModel | undefined;
  allowlist: string[];
  /** Already unique in this session; the caller allocates it. */
  name: string;
  task: string;
  /** The parent's abort: when it fires, the child stops too. */
  signal: AbortSignal | undefined;
  /** Progress while the child runs, throttled by the spawn path itself. */
  onTool: ((child: Child, lastTool: string) => void) | undefined;
}

/** One entry of `run_agents`' `agents` array, after validation. */
interface BatchEntry {
  agent: string;
  task: string;
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
 * S1 of `agent-profiles-defaults.md`: where the host writes the built-in templates,
 * beside this extension (`process::materialize_subagents_extension`). One name, one
 * file, and the same bytes `agents.rs` embeds.
 */
const DEFAULTS_FILE = "agent-profile-defaults.json";

/**
 * Reads `<NABLA_DATA_DIR>/agents.json` — the one file Settings → Subagents writes
 * — and answers the profiles this session may actually use.
 *
 * Read on every tool call, so an edit or a switch change reaches the next
 * delegation without restarting anything. A missing file, an unreadable one or a
 * body that does not parse is an empty store, never an exception: the tools then
 * answer honestly that there is nothing to delegate to (S3), which is what a
 * fresh install with the master switch off looks like.
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
    enabled?: unknown;
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
  // S1 of `agent-profiles-defaults.md`: no profile stored AND the master switch on
  // is the built-in templates, enabled. The empty LIST is the whole condition: a
  // list with anything in it is what the human wrote — including a list of
  // profiles they turned off on purpose, which must stay off — so the defaults
  // never inject themselves beside it.
  if (entries.length === 0 && config.enabled === true) {
    return { defaultModel, profiles: builtInProfiles(dir) };
  }
  return { defaultModel, profiles };
}

/**
 * S1: the built-in templates, read from the file the host materializes beside this
 * extension. An unreadable or malformed file is no defaults at all, not an
 * exception: the tools then say honestly that there is nothing to delegate to,
 * instead of pretending a profile exists. The drift test in `tests/` is what keeps
 * this file and the Settings templates identical.
 */
function builtInProfiles(dir: string): AgentProfile[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(
      readFileSync(join(dir, "extensions", DEFAULTS_FILE), "utf8"),
    );
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const profiles: AgentProfile[] = [];
  for (const entry of parsed) {
    const profile = normalizeProfile(entry);
    if (profile) profiles.push(profile);
  }
  return profiles;
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
/**
 * A name free in this session, following the same suffix rule for both spawning
 * tools: `review`, then `review-2`, … so one session can run the same profile
 * twice without `agent_cancel` becoming a guess. `taken` carries the caller's own
 * claims too, which is what keeps a batch's names apart before any of them is
 * registered.
 */
function pickName(base: string, taken: (candidate: string) => boolean): string {
  const clean = base.trim() === "" ? "agent" : base.trim();
  if (!taken(clean)) return clean;
  for (let suffix = 2; ; suffix += 1) {
    const candidate = `${clean}-${suffix}`;
    if (!taken(candidate)) return candidate;
  }
}

/**
 * S5: the `agents` array as the model sent it, or the sentence that says what is
 * wrong with it. The boundaries live here and not in the schema on purpose: a
 * schema `maxItems` rejection carries pi's own wording, while a refusal can name
 * the entry the model has to fix.
 */
function readBatch(raw: unknown): { refusal: string } | { entries: BatchEntry[] } {
  if (!Array.isArray(raw)) {
    return {
      refusal:
        "`agents` must be an array of `{ agent, task }` entries, so nothing was " +
        `started. Pass between 1 and ${MAX_BATCH} of them.`,
    };
  }
  const entries: unknown[] = raw;
  if (entries.length === 0) {
    return {
      refusal:
        "`agents` was empty, so nothing was started. Pass between 1 and " +
        `${MAX_BATCH} \`{ agent, task }\` entries; for a single task, run_agent ` +
        "is enough.",
    };
  }
  if (entries.length > MAX_BATCH) {
    return {
      refusal:
        `\`agents\` has ${entries.length} entries, and at most ${MAX_BATCH} agents ` +
        `run at once, so nothing was started. Split them into two calls of at ` +
        `most ${MAX_BATCH}.`,
    };
  }
  const seen = new Map<string, number>();
  const batch: BatchEntry[] = [];
  for (let index = 0; index < entries.length; index += 1) {
    const position = index + 1;
    const entry = entries[index] as { agent?: unknown; task?: unknown } | null;
    if (typeof entry !== "object" || entry === null) {
      return {
        refusal:
          `Entry ${position} of \`agents\` is not a \`{ agent, task }\` object, so ` +
          `nothing was started.`,
      };
    }
    const agent = typeof entry.agent === "string" ? entry.agent.trim() : "";
    const task = typeof entry.task === "string" ? entry.task.trim() : "";
    if (agent === "") {
      return {
        refusal:
          `Entry ${position} of \`agents\` names no agent profile, so nothing was ` +
          `started. Every entry needs the profile to run and the task to give it.`,
      };
    }
    if (task === "") {
      return {
        refusal:
          `Entry ${position} of \`agents\` (\`${agent}\`) has an empty \`task\`, so ` +
          `nothing was started. A child cannot be asked what to do later: put the ` +
          `whole task in it.`,
      };
    }
    const previous = seen.get(agent.toLowerCase());
    if (previous !== undefined) {
      return {
        refusal:
          `Entries ${previous} and ${position} of \`agents\` both run \`${agent}\`, ` +
          `so nothing was started. Each entry of one batch must name a different ` +
          `profile: every child needs its own name for agent_status and ` +
          `agent_cancel. Run the same profile in a second call, where its name ` +
          `gets a suffix automatically.`,
      };
    }
    seen.set(agent.toLowerCase(), position);
    batch.push({ agent, task });
  }
  return { entries: batch };
}

/**
 * The first lines of a child's answer, ending in an ellipsis when there is more:
 * a batch summary recognises the work without copying three transcripts into the
 * parent's context.
 */
function excerpt(text: string): string {
  const clean = text.trim();
  if (clean.length <= EXCERPT_CHARS) return clean;
  return `${clean.slice(0, EXCERPT_CHARS).trimEnd()}…`;
}

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
 * The child's real allowlist: the profile's tools, minus our own five (no
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
  const uniqueName = (base: string): string =>
    pickName(base, (candidate) => Boolean(named(candidate)));

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
   * The checks every delegation runs before a child exists: the profile's real
   * allowlist under the human's permission mode, the three-level model resolution
   * (S6 of `agent-profiles.md`) and its effort. A refusal is a sentence the model
   * reads, never an exception — nothing has been created, and the fix is in
   * Settings.
   *
   * Shared by `run_agent` and `run_agents` so the same profile cannot be validated
   * two different ways depending on which tool asked for it.
   */
  function prepareDelegation(
    ctx: Pick<ExtensionContext, "modelRegistry">,
    store: ProfileStore,
    definition: AgentProfile,
  ): { refusal: string } | { allowlist: string[]; model: SessionModel | undefined } {
    const allowlist = childAllowlist(definition);
    if (allowlist.length === 0) {
      const mode = currentMode();
      return {
        refusal:
          `The profile \`${definition.name}\` would give the child no tools ` +
          `under the current "${mode}" permission mode` +
          (definition.tools.length === 0
            ? " (it lists none)"
            : ` (it lists ${definition.tools.join(", ")})`) +
          `, so the child was not started. Give it at least one tool the ` +
          `mode allows, in Settings → Subagents.`,
      };
    }

    // S6: the model resolution order is the profile's own model, then the store's
    // default, then this session's model (`undefined` below).
    const askedModel = definition.model || store.defaultModel;
    const model = resolveModel(ctx, askedModel);
    if (askedModel && !model) {
      return {
        refusal:
          `The profile \`${definition.name}\` asks for model ` +
          `\`${askedModel}\`, which this session cannot reach. Fix that ` +
          `profile's model in Settings → Subagents (or leave it empty to ` +
          `use this session's model) and try again.`,
      };
    }
    if (definition.effort && !EFFORTS.includes(definition.effort.toLowerCase())) {
      return {
        refusal:
          `The profile \`${definition.name}\` asks for effort ` +
          `\`${definition.effort}\`, which is not a thinking level pi has ` +
          `(${EFFORTS.join(", ")}). Fix that profile's effort in Settings → ` +
          `Subagents and try again.`,
      };
    }
    return { allowlist, model };
  }

  /**
   * The one spawn path (S5): an already-validated request becomes a child with its
   * own JSONL, its own name and its own tool list, and the promise settles when
   * that child settles.
   *
   * It never throws — every ending, including a session that cannot even be built,
   * comes back as a `ChildResult` — which is what lets a batch keep the other
   * children's work when one of them fails.
   */
  async function spawnChild(request: SpawnRequest): Promise<ChildResult> {
    const { ctx, parent, definition, model, allowlist, name, task, signal, onTool } =
      request;
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
    // Registered and published before the first await, so `agent_status`, the cap
    // checks and every child of a batch see each other at once.
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
        // five are stripped from the list AND denied, so a profile that names
        // `run_agent` or `run_agents` cannot recurse (S5).
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
      // tool the child used, without copying its conversation anywhere. The
      // caller decides the words; this path only throttles them.
      let lastUpdate = 0;
      const unsubscribe = session.subscribe((event: AgentSessionEvent) => {
        if (event.type !== "tool_execution_start") return;
        if (!onTool) return;
        const at = Date.now();
        if (at - lastUpdate < 250) return;
        lastUpdate = at;
        onTool(child, event.toolName);
      });

      // The parent's tool call being aborted (the human pressed stop) must stop
      // the child too: an orphan child in-process would keep burning tokens. A
      // batch passes the same signal to every child, so one abort ends them all.
      const onAbort = () => {
        void session?.abort().catch(() => undefined);
      };
      signal?.addEventListener("abort", onAbort, { once: true });
      try {
        // `expandPromptTemplates: false`: the task is data, never a command or
        // a template — a child must not be steerable by what the parent writes.
        await session.prompt(task, { expandPromptTemplates: false });
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
          child,
          status: "failed",
          failure: "run",
          text: outcome.text,
          reason: outcome.reason,
        };
      }
      // A child that finished without saying anything has nothing to report, so
      // both callers word it as a failure instead of an empty success. The
      // manifest is deliberately not rewritten here: it recorded `done` when the
      // child settled, exactly as it did before the batch tool existed, and
      // changing that is not this task's decision to make.
      if (outcome.text === "") {
        child.status = "failed";
        return { child, status: "failed", failure: "empty", text: "" };
      }
      return { child, status: "done", text: outcome.text };
    } catch (err) {
      // A failure to even build the child is the child's failure, reported as
      // one: the model must not read a spawn error as "the task is done".
      child.status = "failed";
      child.endedAt = nowIso();
      publish(parent);
      const detail = err instanceof Error ? err.message : String(err);
      return { child, status: "failed", failure: "spawn", text: "", reason: detail };
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
  }

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
      "the enabled ones exist, and when the person configured none, five built-in " +
      "ones do (explorer, researcher, worker, judge, judge-2). The child runs as a " +
      "separate pi session in this " +
      "same process, with only the tools its profile lists and the human's " +
      "permission mode allows, and writes its own session file, so the human can " +
      "watch it in Nabla's sidebar. This tool BLOCKS until the child finishes, " +
      "then returns the child's final message. The child cannot delegate further " +
      "and cannot ask the human anything: what it needs must be in `task`. At " +
      "most 4 children may run at once. For several independent tasks at the " +
      "same time, prefer run_agents: it launches up to 4 children in parallel " +
      "and waits for all of them in one call.",
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

      const ready = prepareDelegation(ctx, store, definition);
      if ("refusal" in ready) {
        return {
          content: [{ type: "text" as const, text: ready.refusal }],
          details: { agent: definition.name, sessionPath: null, status: "failed" },
        };
      }

      const name = params.name ? uniqueName(params.name) : uniqueName(definition.name);
      // One child, awaited: the shared spawn path is the same one a batch uses,
      // so this tool's answer is unchanged while the path is not duplicated.
      const result = await spawnChild({
        ctx,
        parent,
        definition,
        model: ready.model,
        allowlist: ready.allowlist,
        name,
        task: String(params.task ?? ""),
        signal,
        onTool: (child, lastTool) => {
          onUpdate?.({
            content: [
              { type: "text" as const, text: `${name} is running (last tool: ${lastTool})` },
            ],
            details: {
              agent: definition.name,
              name,
              sessionPath: child.sessionPath,
              status: "running",
              lastTool,
            },
          });
        },
      });
      const { child } = result;

      if (result.failure === "run") {
        return {
          content: [
            {
              type: "text" as const,
              text:
                `${name} (${definition.name}) failed: ${result.reason}. Its ` +
                `session file is ${child.sessionPath}${result.text ? `, and its last ` +
                  `words were:\n\n${result.text}` : ""}.${manifestNote()}`,
            },
          ],
          details: {
            agent: definition.name,
            name,
            sessionPath: child.sessionPath,
            status: child.status,
            error: result.reason,
          },
        };
      }
      if (result.failure === "spawn") {
        return {
          content: [
            {
              type: "text" as const,
              text:
                `${name} (${definition.name}) could not run: ${result.reason}. Its ` +
                `session file is ${child.sessionPath}.${manifestNote()}`,
            },
          ],
          details: {
            agent: definition.name,
            name,
            sessionPath: child.sessionPath,
            status: child.status,
            error: result.reason,
          },
        };
      }
      if (result.failure === "empty") {
        return {
          content: [
            {
              type: "text" as const,
              text:
                `${name} (${definition.name}) finished without a final message, ` +
                `so there is nothing to report. Its session file is ` +
                `${child.sessionPath}; read it to see what it did.${manifestNote()}`,
            },
          ],
          details: {
            agent: definition.name,
            name,
            sessionPath: child.sessionPath,
            status: child.status,
          },
        };
      }
      return {
        content: [
          {
            type: "text" as const,
            text: `${result.text}${manifestNote()}`,
          },
        ],
        details: {
          agent: definition.name,
          name,
          sessionPath: child.sessionPath,
          status: child.status,
        },
      };
    },
  });

  pi.registerTool({
    name: "run_agents",
    label: "Run Agents",
    description:
      "Delegate several tasks AT THE SAME TIME: up to 4 agent profiles, all " +
      "launched concurrently and awaited together, so two judges cost one wait " +
      "instead of two. Each entry names a profile the person enabled in Settings " +
      "→ Subagents, or one of the five built-ins that apply when they configured " +
      "none (explorer, researcher, worker, judge, judge-2). Each child is a " +
      "separate pi session in this same process " +
      "with its own profile, its own session file and its own name, using only the " +
      "tools its profile lists and the human's permission mode allows; the human " +
      "can watch each one in Nabla's sidebar. This tool returns ONE result once " +
      "every child has settled: each child's name, its state (done or failed), a " +
      "short excerpt of its answer, and the reason when it failed — a batch where " +
      "one child failed still carries the other children's work. Children cannot " +
      "delegate further and cannot ask the human anything: every decision they " +
      "need must be in their own `task`.",
    promptSnippet: "Run up to 4 agent profiles concurrently and wait for all of them",
    promptGuidelines: [
      "run_agents runs its children concurrently: use it instead of calling run_agent several times when the tasks are independent.",
      "Every entry of one batch must name a different profile, and every task must be self-contained: a child cannot ask the human anything.",
      "The answer summarises every child, including the failed ones: read the failure reasons before retrying anything.",
    ],
    parameters: Type.Object({
      agents: Type.Array(
        Type.Object({
          agent: Type.String({
            description:
              "The agent profile to run, by its name as configured in Settings → Subagents (e.g. `judge`). Different in every entry of one batch.",
          }),
          task: Type.String({
            description:
              "The whole task for that child: what to do, where, and what to answer with. It cannot ask follow-up questions.",
          }),
        }),
        {
          description:
            "1 to 4 agents to run at the same time. Each entry needs its own profile and its own self-contained task.",
        },
      ),
    }),
    async execute(_toolCallId, params, signal, onUpdate, ctx) {
      const parent = parentRefOf(ctx);
      rememberParent(parent);

      // S5: the batch itself, before anything is created. A refusal names the
      // entry to fix instead of failing the whole call on pi's schema wording.
      const batch = readBatch(params.agents);
      if ("refusal" in batch) {
        return {
          content: [{ type: "text" as const, text: batch.refusal }],
          details: { agents: [], status: "failed" },
        };
      }

      const store = loadProfiles();
      if (store.profiles.length === 0) {
        return {
          content: [{ type: "text" as const, text: noProfilesText() }],
          details: { agents: [], status: "failed" },
        };
      }

      // Every entry is resolved before a single child is created, so a batch is
      // never half-launched: one unusable entry refuses the batch and names it.
      const prepared: {
        entry: BatchEntry;
        definition: AgentProfile;
        allowlist: string[];
        model: SessionModel | undefined;
      }[] = [];
      for (let index = 0; index < batch.entries.length; index += 1) {
        const position = index + 1;
        const entry = batch.entries[index];
        const definition = store.profiles.find(
          (candidate) => candidate.name.toLowerCase() === entry.agent.toLowerCase(),
        );
        if (!definition) {
          return {
            content: [
              {
                type: "text" as const,
                text:
                  `Entry ${position} of \`agents\` names \`${entry.agent}\`, which ` +
                  `is not an enabled profile, so the batch was not started. ` +
                  `Enabled profiles: ${describeProfiles(store.profiles)}.`,
              },
            ],
            details: { agents: [], status: "failed" },
          };
        }
        const ready = prepareDelegation(ctx, store, definition);
        if ("refusal" in ready) {
          return {
            content: [
              {
                type: "text" as const,
                text: `Entry ${position} of \`agents\`: ${ready.refusal}`,
              },
            ],
            details: { agents: [], status: "failed" },
          };
        }
        prepared.push({ entry, definition, allowlist: ready.allowlist, model: ready.model });
      }

      // The same session cap run_agent honours, checked synchronously before any
      // child exists, so two batches in one assistant message cannot overshoot it.
      const busy = running();
      if (busy.length + prepared.length > MAX_CHILDREN) {
        return {
          content: [
            {
              type: "text" as const,
              text:
                `This session already has ${busy.length} ` +
                `${busy.length === 1 ? "child" : "children"} running ` +
                `(${busy.map(describe).join("; ")}), so a batch of ` +
                `${prepared.length} would exceed the cap of ${MAX_CHILDREN} ` +
                `children at once, and nothing was started. Wait for one to ` +
                `finish, or cancel one with agent_cancel, then try again.`,
            },
          ],
          details: { agents: [], status: "failed" },
        };
      }

      // Names are allocated before the first spawn so none of the batch's
      // children can collide, and each one is addressable by agent_status.
      const planned: string[] = [];
      const launched = prepared.map((item) => {
        const name = pickName(
          item.definition.name,
          (candidate) => Boolean(named(candidate)) || planned.includes(candidate),
        );
        planned.push(name);
        return { ...item, name };
      });

      const total = launched.length;
      const settledLabels: string[] = [];
      const announce = (text: string) => {
        onUpdate?.({
          content: [{ type: "text" as const, text }],
          details: {
            agents: launched.map((item) => ({ name: item.name, agent: item.definition.name })),
            total,
            settled: settledLabels.length,
          },
        });
      };
      // Honest movement only: how many were started and how many have settled.
      // The children's internals are not streamed here — that is what the
      // session files and agent_status are for.
      announce(
        `Launching ${total} ${total === 1 ? "agent" : "agents"} at the same time: ` +
          `${launched.map((item) => `${item.name} (${item.definition.name})`).join(", ")}.`,
      );

      // S5: concurrent, not sequential. All of them are launched first, then
      // awaited together, and a rejection is turned into that child's failure so
      // it can never discard the others' work.
      const outcomes = await Promise.allSettled(
        launched.map(async (item) => {
          const result = await spawnChild({
            ctx,
            parent,
            definition: item.definition,
            model: item.model,
            allowlist: item.allowlist,
            name: item.name,
            task: item.entry.task,
            signal,
            onTool: undefined,
          });
          settledLabels.push(`${result.child.name} ${result.status}`);
          announce(
            `${settledLabels.length} of ${total} agents settled — ` +
              `${settledLabels.join(", ")}.`,
          );
          return result;
        }),
      );

      const results: ChildResult[] = outcomes.map((outcome, index) => {
        if (outcome.status === "fulfilled") return outcome.value;
        // `spawnChild` reports every ending as a result; this branch exists so a
        // defect there could not swallow the rest of the batch.
        const item = launched[index];
        const detail =
          outcome.reason instanceof Error
            ? outcome.reason.message
            : String(outcome.reason);
        return {
          child: {
            name: item.name,
            agent: item.definition.name,
            sessionPath: "",
            status: "failed",
            startedAt: null,
            endedAt: null,
          },
          status: "failed",
          failure: "spawn",
          text: "",
          reason: detail,
        };
      });

      const done = results.filter((result) => result.status === "done").length;
      const failed = results.length - done;
      const lines: string[] = [
        `${total === 1 ? "1 agent" : `${total} agents`} ran concurrently: ` +
          `${done} finished` +
          (failed > 0 ? `, ${failed} failed` : "") +
          ".",
      ];
      for (const result of results) {
        const child = result.child;
        lines.push("");
        lines.push(`**${child.name}** (${child.agent}) — ${result.status}`);
        if (result.failure === "spawn") {
          lines.push(`Could not run: ${result.reason}.`);
        } else if (result.failure === "run") {
          lines.push(`Failed: ${result.reason}.`);
        } else if (result.failure === "empty") {
          lines.push(
            "Finished without a final message, so there is nothing to report from it.",
          );
        } else {
          lines.push(excerpt(result.text));
        }
        if ((result.failure === "run" || result.failure === "spawn") && result.text !== "") {
          lines.push("", `Its last words were: ${excerpt(result.text)}`);
        }
        lines.push(
          child.sessionPath === ""
            ? "No session file was published for it."
            : `Session file: ${child.sessionPath}`,
        );
      }

      return {
        content: [{ type: "text" as const, text: lines.join("\n") + manifestNote() }],
        details: {
          agents: results.map((result) => ({
            name: result.child.name,
            agent: result.child.agent,
            sessionPath: result.child.sessionPath === "" ? null : result.child.sessionPath,
            status: result.status,
            failure: result.failure ?? null,
            error: result.reason ?? null,
            excerpt: excerpt(result.text),
          })),
        },
      };
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
