# 0003 — Product evolution order

- Status: Accepted
- Scope: Product direction
- Acceptance evidence: `D-006` — general assistant → agent coordination → development environment,
  recorded as an explicit instruction from the maintainer.
- Supersedes / superseded by: none

## Context

Nabla is built by one maintainer with agent workers, against reference desktop-agent repositories
that already contain coordination and development-environment features. Copying those features because
they exist is the main scope risk in this repository. `D-006` fixes the order so a capability's absence
in the current stage is a decision, not an oversight.

## Decision

Evolve in this order, and only with explicit authorization for the next stage:

1. **General desktop assistant** — a working shell, workspaces, sessions, streaming chat with Pi.
2. **Agent coordination** — several sessions/agents working together.
3. **Development environment** — the editor, terminal and toolchain integrations of an IDE.

Work inside the current stage is the default. A later-stage feature must not be built early, even when
a reference project provides it or an ADR already names the seam it will use.

## Alternatives

Building the stages in parallel was rejected: it multiplies unverified surfaces for a single
maintainer and hides whether stage 1 is actually usable. Building the development environment first
was rejected by the explicit order in `D-006`. Stating no order at all was rejected because it leaves
each agent free to choose, which is exactly the drift `D-005`'s documentation layer exists to prevent.

## Consequences

- A stage-2/3 idea surfaces as a documented seam (for example the capability placement map in
  [docs/ai/architecture.md](../ai/architecture.md)) and no empty module folders are created for it.
- Tasks that look small but belong to a later stage require a maintainer decision, recorded in the ODD
  feature log, before implementation.
- "Now" is defined by the active ODD feature document, not by this ADR; this record only orders the
  stages.

## Conditions to revisit

- Stage 1's MVP scope and limits are still open in `D-006`; when they are fixed, they belong in a
  stage-1 feature document, not in this record.
- Moving to stage 2 is a new decision (and will need its own ADR for the coordination model) taken
  when stage 1 is accepted as usable; it supersedes this record rather than editing it.
