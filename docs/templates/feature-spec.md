# Feature specification template

Copy to `odd/tasks/<feature>.md` when a real feature is requested. `odd/` is orchestrator-owned: the
orchestrator creates and maintains the document, workers read it and update only their task row and
the log when authorized. Delete this preamble and every `<placeholder>` in the copy.

The document — not the chat — is the resumable record. Its `## Specs` are authoritative: a quoted
sentence is the requirement, its wording is exact, and it is never rewritten to match what was built.

```markdown
# <feature> — <one-line purpose>

Estado: <en curso | completado>, rama `<branch>` (<stacked on | baseline `main`>).
Alcance: <what this feature covers, in one sentence, and what is explicitly deferred>.

## Specs

- **S1** <Short rule name>: «<verbatim maintainer sentence>» — <what the quote means operationally:
  the observable behavior, the boundary, or the defect it corrects.>
- **S2** <Short rule name>: <requirement stated directly, when there is no maintainer sentence to
  quote. Say which source it comes from.>
- **S3** <Cross-cutting rule>: <input and preconditions | expected output | error semantics |
  observable side effects | what must remain unchanged when the input is rejected>.

## Non-goals

- <Capability deliberately excluded from this feature, with the reason (stage order, open ADR,
  maintainer decision).>

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| T1 | <smallest coherent work unit, files it touches, tests it adds> | S1 | pending | |
| T2 | <next unit; split by behavior, not by file type> | S1, S2 | pending | |

## Log

- **L1** Usuario: «<verbatim sentence that authorized the work>».
- **L2** <Decision or correction taken during implementation, with the reason. Corrections are
  recorded honestly, including the ones that fix an earlier claim.>
```

Guidance for writing the copy:

- Every spec must be checkable. "Works well" is not a spec; "the assistant text grows in the chat while
  the runtime streams" is.
- Quote the maintainer verbatim, in the language it was said, inside the guillemets. The surrounding
  prose is English: repository artifacts are English.
- Keep specs and tasks apart: a spec is behavior, a task is a unit of work that satisfies one or more
  `S#`. A task row cites its specs; the log cites decisions.
- Record still-open questions under the affected spec or in the log, never as an assumed requirement.
- Tests and the documentation that describes changed behavior belong to the task that changes them.
- Never edit a spec to match the implementation. If the built behavior differs, either the code changes
  or the log records that the spec was misread.
