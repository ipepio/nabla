# 0002 — Local product definition stays untracked; `docs/` is the tracked agents layer

- Status: Accepted
- Scope: Documentation / repository policy
- Acceptance evidence: `D-004` (definition in `.docs/`, ignored by Git) and its evolution `D-005`
  (prepare agent instructions, documentation and ADRs once the definition is clear); ODD
  `odd/tasks/agents-architecture.md` `S4` (limits: `.docs/` stays local, new `docs/` is versioned).
- Supersedes / superseded by: none

## Context

The product was defined before any code existed, in Spanish, under `.docs/`. `.gitignore` excludes it
(`/.docs/`) so the definition could be written quickly and rewritten freely. That decision is still
right for the definition, but it left agents with no versioned place to read durable rules: `D-005`
promised that agent-facing documentation would come later, and ODD task A2/A3 is that "later".

## Decision

Keep two documentation layers with different owners:

- `.docs/` — local, Spanish, gitignored product definition and decision register. Read-only
  reference for intent; never committed, never edited from a repository task.
- `docs/` — versioned, English, agent-facing: `docs/ai/*` (deep-dives), `docs/adr/*` (decision
  records), `docs/templates/*` (skeletons). `CONTENTS.md` is the navigation index.
- `odd/tasks/` — orchestrator-owned feature tracking, with quoted specs. Not the same layer: specs
  govern a feature's behavior, ADRs govern durable direction.

Deployment rule: a repository artifact is English; the maintainer's local definition may stay Spanish.
`.docs/` is never a link target from a tracked document, because a clone would have broken links.

## Alternatives

Tracking `.docs/` as-is was rejected: it mixes Spanish product intent with agent instructions and
removes the freedom to revise the definition. Duplicating the definition into `docs/` was rejected:
two copies diverge, and the tracked layer must contain only what agents must obey. Leaving `docs/`
empty until the definition is "final" was rejected by `D-005` itself, which schedules this work.

## Consequences

- The register (`.docs/02-decisiones.md`) is the source for ADR content, but ADRs must restate enough
  context to be readable alone; a clone without `.docs/` still understands the reasoning.
- New durable decisions are promoted here; the register keeps its historical row and does not become
  a second authority.
- The distinction must be stated wherever a document might be mistaken for the other: see
  [docs/ai/architecture.md](../ai/architecture.md) and [CONTENTS.md](../../CONTENTS.md).

## Conditions to revisit

- If the maintainer wants the product definition published, that is a new decision with its own ADR
  (it changes what may be committed and in which language).
- If `docs/` grows past what an agent can load selectively, revisit the index in `CONTENTS.md` rather
  than the layering decided here.
