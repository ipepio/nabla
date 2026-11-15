# ADR template

Copy to the next unused `docs/adr/NNNN-short-decision.md`. Read
[docs/adr/README.md](../adr/README.md) for the conventions; keep the record between 25 and 60 lines.
Delete this preamble and every `<placeholder>` in the copy.

```markdown
# NNNN — <Decision-shaped title, no trailing period>

- Status: <Proposed | Accepted | Provisional | Superseded by NNNN>
- Scope: <Architecture | Persistence | Security | Packaging | Documentation | Product direction>
- Acceptance evidence: <quoted maintainer statement, `D-###` from the local register, and/or the ODD
  specs (`S#` of `odd/tasks/<feature>.md`) this derives from. No decision without a source.>
- Supersedes / superseded by: <links, or `none`>

## Context

<The specific problem that forces a decision. Link the source requirement and the constraints that
actually exist in the repository today: paths, commands, observed behavior. Separate what was
observed from what is assumed. Name the open proposals (`T-###`) or pending tasks that make this
unsettled, if any.>

## Decision

<The chosen behavior or trade-off, precisely enough that an agent can obey it without guessing.
Name real files and commands. If the record is provisional, say which open proposal could replace
it. Do not describe code that does not exist.>

## Alternatives

<Meaningful options and why each was rejected, including keeping the current behavior. Omit this
section only when no alternative was considered, and say so in one line instead of inventing options
to fill the heading.>

## Consequences

<Benefits, costs, operational, data and security impact, failure semantics. State what the decision
forbids. Report only facts the decision implies, not hopes about future quality.>

## Conditions to revisit

<The concrete conditions that make this record obsolete or wrong, and what replaces it: a new
decision, a new ODD feature, a maintainer call. A regulator or helper is not a condition.>

## Verification

<Optional. The command or observation that keeps the record true (`cargo test` inside `src-tauri`,
`npm run build`, a line in `src-tauri/tauri.conf.json`, a path that must exist). ADRs are
documentation; never claim executable enforcement that does not exist.>
```

Rules for the copy: one decision per record; accepted records are never edited, only superseded; link
the ADR from [the index](../adr/README.md), from the affected `docs/ai/*` page and from the ODD task
that implements it.
