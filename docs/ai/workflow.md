# Working workflow

Nabla is tracked with ODD: each feature is a document in `odd/tasks/` holding quoted specs, a task
table and a log. The document — not the chat history — is the resumable record. Sequence:
read → implement → verify → report → record.

## 1. Read the feature document first

- Open `odd/tasks/<feature>.md` in full before touching code. `## Specs` are authoritative: the
  quoted sentences are the requirement and their wording is exact. Never paraphrase a spec into
  something easier to satisfy.
- The `## Tasks` table (ID, specs, state, commit) is the checklist and the work-unit plan.
- `## Log` entries (`L#`) record decisions and corrections. Read them before repeating a rejected
  idea, and add an entry when you make a consequential decision.

## 2. Scope before writing

Establish whether writing is authorized; investigation stays read-only. If a requirement, an owned
surface or a product choice is ambiguous, ask once, with a concrete proposal. Do not invent a spec,
and do not widen the task because an adjacent defect is visible — report it instead.

## 3. Implement as a work unit

- The smallest coherent change that satisfies the assigned `S#`: code, its tests, and the docs that
  describe the changed behavior.
- Apply [component architecture](component-architecture.md) for UI and [conventions](conventions.md)
  for code. Reuse the existing module that already does the job instead of re-implementing it.
- Keep one writer per surface. Parallel work is allowed only with disjoint edit surfaces (the
  foundation used `src-tauri/src/**` and `src/**` in parallel); overlapping writers corrupt intent.
- Follow the ODD test-first expectation in [verification](verification.md). A behavior change without
  a test needs a stated reason, not silence.

## 4. Verify

Run the exact commands from [verification](verification.md) and report observed output. Cross-cutting
features get an independent verification pass (a separate worker or a later review) that does not
trust the implementer's summary; the implementer never signs off on its own work. Note that GUI
acceptance (`npm run tauri dev`) is manual and often pending.

## 5. Commit as reviewable slices

Conventional Commits, one work unit per commit, about 400 authored lines maximum. Tests and docs
travel with the code they describe. Commit only with explicit permission; the maintainer or the
orchestrator runs the terminal git commands. If a unit would exceed the slice, split it by behavior,
not by file type.

## 6. Record and report

- Update the `## Tasks` row (state, commit) and add a `## Log` entry for decisions or corrections.
  Specs are never edited to match what was built.
- Report in this shape: outcome, files touched, verification (exact commands and results), spec
  coverage (`S#` satisfied, `S#` not satisfied), risks or open decisions, next action.
- If work is interrupted, leave the task table and log accurate enough that a fresh session can
  resume from the document alone. Memory tools supplement this record; they never replace it.

## 7. Durable knowledge

A discovery that will change future work — a constraint, a non-obvious root cause, an accepted rule —
belongs in `docs/ai/*` (agent-facing, versioned) or in a decision record, not only in a task log.
`.docs/` is the maintainer's local product definition: read it for intent, never commit it.
