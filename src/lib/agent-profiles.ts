// Subagent profiles: the draft the Settings → Subagents pane edits, the tools a
// profile may list, and the five templates that pane offers on demand (S2, S4 and
// S6 of `odd/tasks/agent-profiles.md`).
//
// The state lives at module scope, like `preferences.ts` and `provider-models.ts`,
// for one concrete reason: the dialog renders the pane through a `v-if` chain, so
// switching to another section unmounts it. A draft held inside the component
// would be thrown away by that unmount, and the human would lose edits just by
// looking at another pane. Keeping it here means the editor survives a section
// switch AND a dialog reopen; nothing is written to `agents.json` until the human
// presses Save.
//
// Three rules shape the module:
//
// 1. **The draft is the only thing edited.** Every mutation below changes the
//    draft and nothing else. `saveAgentsDraft` is the single write, through
//    `save_agents_config`, and its answer replaces the draft (the file is the
//    truth after a save, exactly like `save_settings`).
// 2. **A read never eats unsaved work.** `loadAgentsConfig` re-reads the file on
//    every pane mount, but it keeps the draft when it holds edits (`dirty`). A
//    human edit of `agents.json` is still picked up whenever the draft is clean,
//    and `discardAgentsChanges` is the explicit way to throw the draft away.
// 3. **Nothing is created on the human's behalf.** No profile and no template is
//    added by mounting, reading or saving: the templates are data this module
//    hands out, and they only join the draft when `addAgentProfile` is called.
//
// The templates are also the fallback for an empty store (S1 of
// `agent-profiles-defaults.md`): with the master switch on and no profile saved,
// these five are what a child runs with, enabled, without being added here. The
// TEXT is written once, in this file, and mirrored as data in
// `src-tauri/assets/agent-profile-defaults.json` — the asset the Rust gate embeds
// and the host writes beside the subagent extension. Editing a template means
// editing that asset in the same commit: `tests/agent-profile-defaults.test.ts`
// fails when names, instructions, tool lists or descriptions disagree.
//
// No `watch` anywhere: the pane drives every change through an explicit handler,
// and `dirty` is a computed over the draft and the last file answer, so no getter
// runs during setup over state declared below it.
import { computed, ref } from "vue";
import {
  commandErrorMessage,
  getAgentsConfig,
  saveAgentsConfig,
} from "@/contracts/commands";
import type { AgentProfile, AgentsConfig } from "@/contracts/types";
import type { MessageKey } from "@/lib/i18n";

// ── the tool catalogue ──────────────────────────────────────────────────────

/**
 * The tool names a profile may list: the same names the extensions that already
 * gate and build a child use, in the order those files introduce them —
 * `read`/`grep`/`find`/`ls` from `resources/nabla-permissions.ts` (`FREE_TOOLS`),
 * `edit`/`write` from `FILE_TOOLS`, `bash`/`powershell` from `SHELL_TOOLS` in both
 * `nabla-permissions.ts` and `resources/nabla-subagents.ts`, plus `web_search`,
 * the agent's search tool (S1 of `odd/tasks/web-search.md`).
 *
 * It is a catalogue of NAMES, not a second permission model: nothing here decides
 * whether a tool runs. The permission mode still strips the shell and
 * file-writing families from a child's allowlist at spawn time, and a name this
 * build does not know is kept in the file untouched (the extension strips what it
 * cannot grant, never the reverse).
 */
export interface AgentToolOption {
  /** The tool name as pi and the extensions spell it. Never translated. */
  id: string;
  /** One line of what the tool does, shown next to the checkbox. */
  descriptionKey: MessageKey;
}

export const AGENT_TOOLS: readonly AgentToolOption[] = [
  { id: "read", descriptionKey: "settings.subagents.tool.read" },
  { id: "grep", descriptionKey: "settings.subagents.tool.grep" },
  { id: "find", descriptionKey: "settings.subagents.tool.find" },
  { id: "ls", descriptionKey: "settings.subagents.tool.ls" },
  { id: "edit", descriptionKey: "settings.subagents.tool.edit" },
  { id: "write", descriptionKey: "settings.subagents.tool.write" },
  { id: "bash", descriptionKey: "settings.subagents.tool.bash" },
  { id: "powershell", descriptionKey: "settings.subagents.tool.powershell" },
  { id: "web_search", descriptionKey: "settings.subagents.tool.webSearch" },
];

/** The known names, for a row that renders chips in catalogue order. */
const TOOL_IDS: readonly string[] = AGENT_TOOLS.map((tool) => tool.id);

/**
 * One profile's tools in catalogue order. A name this build does not know is kept
 * after the known ones, because that is what the file holds and what the
 * extension will honour; dropping it here would let one checkbox erase it.
 */
export function orderedTools(tools: readonly string[]): string[] {
  const known = TOOL_IDS.filter((id) => tools.includes(id));
  const unknown = [
    ...new Set(tools.filter((tool) => !TOOL_IDS.includes(tool))),
  ];
  return [...known, ...unknown];
}

// ── the templates (S2) ──────────────────────────────────────────────────────

/**
 * What a template fills in, in one line: the UI half. The profile itself is data
 * written into `agents.json` and read by a model, so its description and
 * instructions stay in English like every other prompt this app ships.
 *
 * No template names a model: a model id has to resolve against the account's own
 * catalogue, and inventing one would produce a profile that fails at spawn time.
 * The description says which KIND of model the profile wants (a cheap one, the
 * strongest one, one from another family) and leaves the pick to the human, which
 * is also what S6's resolution order expects (profile → app default → parent).
 */
export interface AgentProfileTemplate {
  /** Stable slug, also the id the pane keys the offer by. */
  id: string;
  /** One line: when to reach for this profile, in the human's words. */
  purposeKey: MessageKey;
  /** The profile the click adds, fully written and DISABLED (S3). */
  profile: AgentProfile;
}

/**
 * The judge's instructions, shared by both judges on purpose: the two runs are
 * only comparable if they are asked the same question in the same words, and the
 * difference between them is the model family, not the prompt.
 */
const JUDGE_INSTRUCTIONS = [
  "You judge work against what was asked, not against your own taste.",
  "",
  "First find what defines \"right\" for this task: the request, the spec, the tests",
  "or the instructions the work was supposed to follow. Then check the change",
  "against them, one requirement at a time.",
  "",
  "Answer with the problems, worst first, each naming the file and the line it",
  "lives at and the requirement it breaks. Say plainly when the work does meet the",
  "spec. Do not invent problems to look thorough.",
].join("\n");

/** The five profiles S2 offers: written, read-only until clicked, off by default. */
export const AGENT_PROFILE_TEMPLATES: readonly AgentProfileTemplate[] = [
  {
    id: "explorer",
    purposeKey: "settings.subagents.template.explorer",
    profile: {
      name: "explorer",
      description:
        "Where is it: locates the files, symbols and definitions that answer a " +
        "question about this repository and reports paths with line numbers. " +
        "Read-only. Use a cheap, fast model.",
      tools: ["read", "grep", "find"],
      model: null,
      effort: "low",
      instructions: [
        "You find things in this repository. You read, you search, you report.",
        "",
        "You never change a file, never run a command and never propose a patch:",
        "your answer is WHERE the thing is. Answer with the paths and line numbers",
        "that matter, one line each saying what is there, and stop. If the thing is",
        "not in the repository, say so instead of guessing at a path.",
      ].join("\n"),
      enabled: false,
    },
  },
  {
    id: "researcher",
    purposeKey: "settings.subagents.template.researcher",
    profile: {
      name: "researcher",
      description:
        "What is true outside this repository: documentation, release notes and " +
        "upstream projects, with a link for every claim. Read-only.",
      tools: ["read", "grep", "find", "web_search"],
      model: null,
      effort: "medium",
      instructions: [
        "You answer questions about what is true OUTSIDE this repository:",
        "libraries, versions, APIs, and what other projects do.",
        "",
        "Use the web search tool. Read the repository only when it clarifies how",
        "this project uses the thing you looked up. Give one link per claim, and",
        "never state a fact you could not read: when you cannot find it, say it was",
        "not found instead of filling the gap with what is usually true.",
      ].join("\n"),
      enabled: false,
    },
  },
  {
    id: "worker",
    purposeKey: "settings.subagents.template.worker",
    profile: {
      name: "worker",
      description:
        "Change it: implements one task in this repository, with tests, and runs " +
        "them. Use the strongest model you have.",
      tools: ["read", "grep", "find", "edit", "write", "bash"],
      model: null,
      effort: "high",
      instructions: [
        "You change code in this repository.",
        "",
        "Read before you write. Keep the change as small as the task allows. Follow",
        "the conventions of the files you touch instead of the ones you prefer. Run",
        "the test, the build or the command that covers what you changed.",
        "",
        "Answer with what you changed, the command you ran and what it printed.",
        "Nothing is done until you have run something: a change you did not check is",
        "a guess, and you say so when you could not check it.",
      ].join("\n"),
      enabled: false,
    },
  },
  {
    id: "judge",
    purposeKey: "settings.subagents.template.judge",
    profile: {
      name: "judge",
      description:
        "Is it right against the spec: reviews a change or a plan and reports " +
        "where it does not meet what was asked. Read-only. Use a strong model.",
      tools: ["read", "grep", "find"],
      model: null,
      effort: "high",
      instructions: JUDGE_INSTRUCTIONS,
      enabled: false,
    },
  },
  {
    id: "judge-2",
    purposeKey: "settings.subagents.template.judgeSecond",
    profile: {
      name: "judge-2",
      description:
        "Is it right against the spec, judged blind: the same question and the " +
        "same instructions as judge, with a model from another family, so the two " +
        "answers are independent. Read-only.",
      tools: ["read", "grep", "find"],
      model: null,
      effort: "high",
      instructions: JUDGE_INSTRUCTIONS,
      enabled: false,
    },
  },
];

// ── the draft and the last file answer ──────────────────────────────────────

/** The off state, which is also what a fresh install reads (S3). */
function emptyConfig(): AgentsConfig {
  return { enabled: false, modelDefault: null, profiles: [] };
}

/** Deep copy through JSON: every value of this DTO is JSON, so nothing is lost. */
function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

/**
 * What the file answered last, or `null` before the first read. The draft below is
 * compared against it, so "unsaved changes" is a fact about the two, not a flag
 * somebody has to remember to set in every handler.
 */
const fileConfig = ref<AgentsConfig | null>(null);

/**
 * The draft the pane renders and edits. `null` only before the first read; the
 * pane paints its loading line for that, never a half-built config.
 */
export const agentsConfig = ref<AgentsConfig | null>(null);

/** A read is on its way. */
export const agentsConfigLoading = ref(false);

/** A save is on its way: the pane's buttons are inert meanwhile. */
export const agentsConfigSaving = ref(false);

/**
 * Why the read failed, verbatim. A missing file is NOT an error (Rust answers the
 * off state), so this is only ever a real problem: an unparseable file, or a data
 * directory the app cannot read.
 */
export const agentsConfigReadError = ref<string | null>(null);

/**
 * Why the save was refused, verbatim as Rust wrote it: it names the profile and
 * what is wrong with it (a missing description, a duplicate name, an effort that
 * is not a thinking level, a model id that cannot be one). Never reworded here —
 * the human reads the sentence that tells them what to fix.
 */
export const agentsConfigSaveError = ref<string | null>(null);

/**
 * A save landed since the last read, so the pane can say so. Cleared by the next
 * edit, because "Saved" must stop being true the moment the draft moves again.
 */
export const agentsConfigSaved = ref(false);

/**
 * One normal form of the store, for comparing the draft with the file: field by
 * field and in a fixed order, so a re-serialization that reorders keys is not
 * mistaken for an edit. `null` and `""` are kept apart on purpose (both are
 * meaningful for `model` and `effort`).
 */
function fingerprint(config: AgentsConfig): string {
  return JSON.stringify([
    config.enabled,
    config.modelDefault,
    config.profiles.map((profile) => [
      profile.name,
      profile.description,
      profile.tools,
      profile.model,
      profile.effort,
      profile.instructions,
      profile.enabled,
    ]),
  ]);
}

/** The draft holds edits that are not in the file yet. */
export const agentsConfigDirty = computed<boolean>(() => {
  const draft = agentsConfig.value;
  const file = fileConfig.value;
  if (draft === null || file === null) return false;
  return fingerprint(draft) !== fingerprint(file);
});

// ── reading and writing ─────────────────────────────────────────────────────

/**
 * Reads the store. Called when the pane mounts, which happens every time the
 * section is selected; the read is skipped while the draft holds unsaved edits, so
 * looking at another pane and coming back cannot lose them. `force` is the
 * explicit "throw my edits away" path (`discardAgentsChanges`).
 *
 * It never rejects: a failed read keeps whatever draft is on screen and reports
 * the reason. With no draft at all it installs the off state, so the pane renders
 * the switches and an empty list instead of an error page — an unreadable file is
 * something the human can fix by writing a valid one here.
 */
export async function loadAgentsConfig(
  options: { force?: boolean } = {},
): Promise<void> {
  if (!options.force && agentsConfigDirty.value) return;
  agentsConfigLoading.value = true;
  agentsConfigReadError.value = null;
  try {
    const config = await getAgentsConfig();
    fileConfig.value = config;
    agentsConfig.value = clone(config);
    agentsConfigSaveError.value = null;
    agentsConfigSaved.value = false;
  } catch (cause) {
    agentsConfigReadError.value = commandErrorMessage(cause);
    if (agentsConfig.value === null) {
      fileConfig.value = emptyConfig();
      agentsConfig.value = emptyConfig();
    }
  } finally {
    agentsConfigLoading.value = false;
  }
}

/**
 * The single write. The whole draft goes to `save_agents_config`, which validates
 * first and answers what the file now holds; the draft is replaced by that answer
 * (so a value Rust normalized is what stays on screen) and the error line is
 * cleared.
 *
 * A refused save changes nothing on disk and keeps the draft exactly as it was,
 * with Rust's sentence on screen. The read error is cleared too: a save that lands
 * proves the file is readable now.
 */
export async function saveAgentsDraft(): Promise<void> {
  const draft = agentsConfig.value;
  if (draft === null || agentsConfigSaving.value) return;
  agentsConfigSaving.value = true;
  agentsConfigSaveError.value = null;
  agentsConfigSaved.value = false;
  try {
    const answer = await saveAgentsConfig(draft);
    fileConfig.value = answer;
    agentsConfig.value = clone(answer);
    agentsConfigReadError.value = null;
    agentsConfigSaved.value = true;
  } catch (cause) {
    agentsConfigSaveError.value = commandErrorMessage(cause);
  } finally {
    agentsConfigSaving.value = false;
  }
}

/** Throws the draft away and reads the file again: the explicit undo. */
export async function discardAgentsChanges(): Promise<void> {
  await loadAgentsConfig({ force: true });
}

// ── a profile and its fields ────────────────────────────────────────────────

/** A profile with every field at its documented default: nothing enabled (S3). */
export function blankProfile(): AgentProfile {
  return {
    name: "",
    description: "",
    tools: [],
    model: null,
    effort: null,
    instructions: "",
    enabled: false,
  };
}

/** The draft's profile at `index`, or `null` when it is gone (a removal). */
export function profileAt(index: number): AgentProfile | null {
  return agentsConfig.value?.profiles[index] ?? null;
}

/**
 * Adds one profile to the draft, at the end, and answers its index so the caller
 * can open its editor. The profile arrives disabled unless the caller says
 * otherwise: this is the only path that creates one, and it is only called from an
 * explicit click.
 */
export function addAgentProfile(profile: AgentProfile): number | null {
  const draft = agentsConfig.value;
  if (draft === null) return null;
  draft.profiles.push(clone(profile));
  agentsConfigSaved.value = false;
  return draft.profiles.length - 1;
}

/**
 * Removes one profile from the DRAFT. Nothing is deleted on disk until the human
 * saves, which is what makes a mistaken removal recoverable by not saving.
 */
export function removeAgentProfile(index: number): void {
  const draft = agentsConfig.value;
  if (draft === null) return;
  draft.profiles = draft.profiles.filter((_, at) => at !== index);
  agentsConfigSaved.value = false;
}

/**
 * Writes one field of one profile. The field list is the DTO's, no more: a patch
 * with a key the store does not have is ignored instead of being written into
 * `agents.json`.
 */
export function updateAgentProfile(
  index: number,
  patch: Partial<AgentProfile>,
): void {
  const profile = profileAt(index);
  if (profile === null) return;
  if (patch.name !== undefined) profile.name = patch.name;
  if (patch.description !== undefined) profile.description = patch.description;
  if (patch.tools !== undefined) profile.tools = orderedTools(patch.tools);
  if (patch.model !== undefined) profile.model = patch.model;
  if (patch.effort !== undefined) profile.effort = patch.effort;
  if (patch.instructions !== undefined)
    profile.instructions = patch.instructions;
  if (patch.enabled !== undefined) profile.enabled = patch.enabled;
  agentsConfigSaved.value = false;
}

/** S3: the master switch, on the draft. Off means no delegation at all. */
export function setSubagentsEnabled(next: boolean): void {
  const draft = agentsConfig.value;
  if (draft === null) return;
  draft.enabled = next;
  agentsConfigSaved.value = false;
}

/** S6, second level: the model a profile uses when it names none. */
export function setSubagentsModelDefault(next: string | null): void {
  const draft = agentsConfig.value;
  if (draft === null) return;
  draft.modelDefault = next === null || next.trim() === "" ? null : next.trim();
  agentsConfigSaved.value = false;
}

/**
 * A template already in the list, matched the way Rust matches names (trimmed,
 * case-insensitive). The pane uses it to refuse adding the same profile twice
 * instead of creating a pair the save would reject.
 */
export function hasProfileNamed(name: string): boolean {
  const wanted = name.trim().toLowerCase();
  const draft = agentsConfig.value;
  if (draft === null) return false;
  return draft.profiles.some(
    (profile) => profile.name.trim().toLowerCase() === wanted,
  );
}
