// The built-in subagent profiles have one source of truth
// (`odd/tasks/agent-profiles-defaults.md`, S1).
//
// When the master switch is on and no profile is stored, the five templates
// Settings offers still arrive enabled. Three consumers need that text, and only
// one of them may own it:
//
//   - `src/lib/agent-profiles.ts` — the templates the Settings pane shows, in
//     English, written once;
//   - `src-tauri/assets/agent-profile-defaults.json` — the same profiles as data,
//     embedded into the Rust mirror (`agents.rs`) so `delegation_active()` and
//     `enabled_profiles()` answer for the fallback, and materialized beside the
//     subagent extension so `run_agent`/`run_agents` can offer them;
//   - `resources/nabla-subagents.ts` — the extension that reads that file at every
//     tool call.
//
// Rust and the extension both read the JSON, so the only place the prose can drift
// is between TypeScript and the asset. That is what this file pins: names,
// descriptions, tool lists, effort, model and instructions, field by field. A
// template edited in one place and not the other fails here instead of shipping two
// different `worker` instructions.
//
// `agent-profiles.ts` reaches the Tauri seam and `vue`; neither is what this file
// exercises, so the seam is answered by an inline stub module (a data: url, so no
// extra file is added to `tests/support/`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(import.meta.dirname, "..");

/** The asset both the Rust mirror and the extension read. */
const DEFAULTS_ASSET = path.join(
  projectRoot,
  "src-tauri",
  "assets",
  "agent-profile-defaults.json",
);

const stubSource = [
  'export function commandErrorMessage(error) { return String(error); }',
  'export async function getAgentsConfig() { throw new Error("stub: no bridge"); }',
  'export async function saveAgentsConfig() { throw new Error("stub: no bridge"); }',
].join("\n");
const stubUrl = `data:text/javascript,${encodeURIComponent(stubSource)}`;

registerHooks({
  resolve(specifier, context, nextResolve) {
    // The module's only connection to Rust: nothing here reads or writes the
    // store, so an inert module is enough to load the templates themselves.
    if (specifier === "@/contracts/commands") {
      return { url: stubUrl, shortCircuit: true };
    }
    // The bundle aliases `@/` to `src/` and resolves extensionless relative
    // imports; both rules are applied here as Vite does, and only when the `.ts`
    // sibling really exists — a dependency's own CommonJS require is not ours to
    // rewrite.
    if (specifier.startsWith("@/")) {
      return nextResolve(
        `${new URL("src/", `file://${projectRoot}/`).href}${specifier.slice(2)}.ts`,
        context,
      );
    }
    if (specifier.startsWith(".") && path.extname(specifier) === "") {
      const candidate = new URL(`${specifier}.ts`, context.parentURL);
      if (existsSync(fileURLToPath(candidate))) return nextResolve(candidate.href, context);
    }
    return nextResolve(specifier, context);
  },
});

const { AGENT_PROFILE_TEMPLATES } = await import(
  "../src/lib/agent-profiles.ts"
);

interface AssetProfile {
  name: string;
  description: string;
  tools: string[];
  model: string | null;
  effort: string | null;
  instructions: string;
  enabled: boolean;
}

/** The asset as the JSON parser and the Rust mirror both see it. */
function assetProfiles(): AssetProfile[] {
  const parsed: unknown = JSON.parse(readFileSync(DEFAULTS_ASSET, "utf8"));
  assert.ok(
    Array.isArray(parsed),
    "the asset is the profiles array: it carries no master switch",
  );
  return parsed as AssetProfile[];
}

// S1: the asset is the templates the Settings pane offers, in the same order and
// with the same words. Names, instructions and tool lists are compared as they
// are stored; a template that grows a field the asset does not carry fails here.
test("the embedded defaults are exactly the templates Settings offers", () => {
  const asset = assetProfiles();
  const templates = AGENT_PROFILE_TEMPLATES.map((template) => template.profile);

  assert.deepEqual(
    asset.map((profile) => profile.name),
    templates.map((profile) => profile.name),
    "the defaults are the templates, in template order",
  );

  for (let index = 0; index < templates.length; index += 1) {
    const expected = templates[index];
    const actual = asset[index];
    assert.equal(
      actual.instructions,
      expected.instructions,
      `the instructions of \`${expected.name}\` must be the template's, word for word`,
    );
    assert.deepEqual(
      actual.tools,
      expected.tools,
      `the tool list of \`${expected.name}\` must be the template's`,
    );
    assert.equal(
      actual.description,
      expected.description,
      `the description of \`${expected.name}\` must be the template's`,
    );
    assert.equal(
      actual.effort,
      expected.effort,
      `the effort of \`${expected.name}\` must be the template's`,
    );
    assert.equal(
      actual.model,
      expected.model,
      `the model of \`${expected.name}\` must be the template's`,
    );
  }
});

// S1: the defaults arrive ENABLED — that is the whole point of the fallback — and
// they are written in English like every other prompt this app ships.
test("the defaults arrive enabled and fully written", () => {
  const asset = assetProfiles();
  assert.equal(asset.length, AGENT_PROFILE_TEMPLATES.length);
  for (const profile of asset) {
    assert.equal(profile.enabled, true, `\`${profile.name}\` arrives enabled`);
    assert.notEqual(profile.description.trim(), "");
    assert.notEqual(profile.instructions.trim(), "");
    assert.ok(profile.tools.length > 0, `\`${profile.name}\` lists tools`);
    assert.match(
      profile.name,
      /^[a-z][a-z0-9-]*$/,
      "a name is the handle `run_agent` takes, so it stays a slug",
    );
  }
});

// S1: the seam itself, so no consumer can quietly start carrying its own copy. The
// Rust mirror embeds this one file, the host materializes it beside the extension,
// and the extension reads it from there on every tool call.
test("one asset feeds the Rust mirror and the extension that reads it", () => {
  const agentsRs = readFileSync(
    path.join(projectRoot, "src-tauri", "src", "agents.rs"),
    "utf8",
  );
  assert.match(
    agentsRs,
    /include_str!\("\.\.\/assets\/agent-profile-defaults\.json"\)/,
    "agents.rs embeds the asset at build time",
  );

  const processRs = readFileSync(
    path.join(projectRoot, "src-tauri", "src", "process", "mod.rs"),
    "utf8",
  );
  assert.match(
    processRs,
    /agents::defaults_source\(\)/,
    "the host materializes the same bytes the mirror embeds",
  );

  const extension = readFileSync(
    path.join(projectRoot, "resources", "nabla-subagents.ts"),
    "utf8",
  );
  assert.ok(
    /const DEFAULTS_FILE = "agent-profile-defaults\.json";/.test(extension),
    "the extension names the materialized defaults file",
  );
  assert.ok(
    /join\(dir, "extensions", DEFAULTS_FILE\)/.test(extension),
    "the extension reads the defaults from `<data_dir>/extensions`, not a copy of its own",
  );
});
