# Agent profiles: on with no profiles means defaults
One-behavior change on top of the current `agents.rs` gate.

## Specs
S1 «deberíamos de hacer que si no hay perfiles configurados, que tome los default y dejar eso» — when the master switch is on and the store holds no enabled profile, the five built-in templates arrive enabled (explorer, researcher, worker, judge, judge-2), exactly as the Settings templates define them. Store content wins: any user-defined or user-disabled profile makes the store authoritative and defaults never reappear.

## Tasks

| Task | Specs | Work | Commit |
| --- | --- | --- | --- |
| T1 | S1 | The fallback in the Rust gate (`agents.rs`) and in the extension's own gate, the JSON asset both read, the Node drift test against `src/lib/agent-profiles.ts`, Rust tests, the Settings note, the README. Spawn-time gate semantics kept; no publishing. | not committed |

## Log
L1 «no, deberíamos de hacer que si no hay perfiles configurados, que tome los default y dejar eso (si es como dices)» — the maintainer wants delegation active with master on and zero profiles, resolving today's improvisation-with-bash case.

L2 T1 decisions. **The condition is the empty LIST, not "nothing enabled"**: a store whose profiles are all off is what the human wrote, so it stays authoritative and the defaults never inject beside it (S1's last sentence). **The template text has one source**: `src/lib/agent-profiles.ts`, mirrored as data in `src-tauri/assets/agent-profile-defaults.json`; `agents.rs` embeds it (`include_str!`) and `materialize_subagents_extension` writes the same bytes beside the extension, so neither Rust nor the extension carries prose of its own, and `tests/agent-profile-defaults.test.ts` fails if the asset and the templates disagree. `src-tauri/src/process/mod.rs` is touched for that materialization only — the spawn seam needs nothing else, `NABLA_DATA_DIR` already tells the extension where to read. The Settings pane keeps the empty list editable and states that delegation is running with the defaults, because an empty-looking list beside spawning children would misdescribe the app.
