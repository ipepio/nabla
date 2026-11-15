# Maintainer preferences

Separate what the maintainer actually established (evidence: quoted requests recorded in
`odd/tasks/*.md` logs and the local `.docs/` decision register) from defaults proposed here.

## Confirmed

| Preference | Evidence |
| --- | --- |
| Speak the maintainer's language; he writes Spanish and mixes in English (“please”) | Prompts quoted in `odd/tasks/nabla-foundation.md` (L1) and `odd/tasks/agents-architecture.md` (L1) |
| Repository artifacts are written in English; `.docs/` stays Spanish and gitignored (D-004) | `odd/tasks/agents-architecture.md` S4, `.gitignore` entry `/.docs/` |
| Keep the user's own words as the spec — quote, never paraphrase | `odd/tasks/*.md` `## Specs` sections quote the maintainer verbatim |
| Adapt a reference architecture instead of copying it | `odd/tasks/agents-architecture.md` S2: «adaptar, no copiar crudo» |
| Decide big scope shifts with him before building | `odd/tasks/nabla-foundation.md` L1 («¿Largamos con el shell + store?» → «dale») |
| Vue + Vuetify with dark theme by default, CSP enabled from day one | `odd/tasks/nabla-foundation.md` S1, L5; `src/main.ts`; `src-tauri/tauri.conf.json` |
| Work in reviewable slices; a commit that balloons past ~400 lines is a mistake to correct | `odd/tasks/nabla-foundation.md` L7 |
| Report a correction honestly instead of quietly fixing the record | `odd/tasks/nabla-foundation.md` L4 |
| Reference projects inform patterns; their defects are not inherited | `.docs/03-referencias.md` precautions; D-010 |

## Proposed defaults — not yet confirmed

| Topic | Default |
| --- | --- |
| Tone | Direct and concise; explain consequential trade-offs in one or two sentences, then act |
| Autonomy | Complete authorized in-scope work without asking at each step; stop at consequential decisions |
| Architecture | Incremental improvement over rewrites; no speculative folders, layers or abstractions |
| Dependencies | Reuse declared dependencies; ask before adding or upgrading |
| Git | No commit, push, rebase or PR without explicit permission; Conventional Commits when permitted |
| Verification | Never claim a check ran; report the exact command and the observed result |
| Completion report | Outcome, files touched, observed checks, remaining risks or open decisions |

## Boundaries the maintainer set explicitly

- `.docs/` is local product definition; `docs/` is the versioned agent documentation.
- `odd/` belongs to the orchestrating workflow; feature docs there are not free-form.
- Pi stays inside the app: the user never sees or manages a separate process (`odd/tasks/pi-runtime.md` S1).

## Preference updates

When a durable preference is established, add it to the first table with its evidence. Distinguish
a one-off instruction from a standing rule; when in doubt, treat it as one-off and ask during the next
relevant task instead of promoting it silently.

## To refine later

- Expected depth of explanations and frequency of progress updates.
- Branch, review and merge policy per feature.
- UI acceptance expectations (what the maintainer personally checks versus what an agent can prove).
- How much environment setup an agent may perform autonomously.
