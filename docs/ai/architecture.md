# Architecture reference

Stack: Tauri v2 host (Rust 2024 edition) + Vue 3 + Vuetify 4 webview (TypeScript strict, Vite 8) on
top of Pi (`@earendil-works/pi-coding-agent`). App identifier `com.ipepio.nabla`, product `Nabla`.

## Process tree

```text
Nabla (one OS app, no external daemon)
├── Tauri host — Rust, src-tauri/src/
│   ├── commands.rs          the only seam to the webview
│   ├── store.rs             nabla.db (SQLite/rusqlite): workspaces + session index
│   ├── settings.rs          settings.json: human-editable preferences
│   └── runtime supervisor   PLANNED — see odd/tasks/pi-runtime.md
│       ├── pi child A       headless, one per session, NDJSON over stdin/stdout
│       └── pi child B       dies with the app; no visible terminal
└── WebView — Vue, src/
    ├── contracts/           invoke() wrappers + DTO mirrors  →  commands.rs
    └── App.vue              shell views
```

There is no path from the webview to pi. Commands flow Vue → `src/contracts/commands.ts` → Tauri
command → store/settings, and (once the runtime lands) events flow pi → Rust → typed Tauri events →
Vue.

## Module map

| Module | Current path | Owns | Must not own |
| --- | --- | --- | --- |
| Vue interface | `src/App.vue`, `src/main.ts` | Conversations, activity, configuration UI | Privileged tools, secrets, direct process access |
| IPC contracts | `src/contracts/` | Operations and payload shapes that cross processes | Generic application logic |
| Tauri host | `src-tauri/src/lib.rs` | Native integration, state wiring, supervision | Re-implementing the agent loop |
| Command layer | `src-tauri/src/commands.rs` | Validation, store/settings calls, error mapping | Domain rules that belong to the store |
| App store | `src-tauri/src/store.rs` | Owned projection: workspaces, session index | Transcript content, pi's session files |
| Human config | `src-tauri/src/settings.rs` | `settings.json` load/save | Accumulated application state |
| Image input | `src-tauri/src/image_input.rs` | Format sniffing, base64 and the per-turn bounds of a prompt's images | Reading files the human did not pick or drop; persisting a payload |
| Runtime | not created | Spawning/stopping pi sessions, streaming events | Depending on Vue or presentation details |

## Capability placement

This section is the map future capabilities refer to. They follow one rule: each capability grows
where its owner lives, behind the contracts seam, using Pi's public interfaces.

| Capability | Intended home | Constraint |
| --- | --- | --- |
| pi runtime / sessions | `src-tauri/src/` modules + `src/contracts/` mirrors | One headless process per session, host-supervised |
| MCP integrations | runtime module owning lifecycle + UI panel | Config, credentials and trust are not frontend concerns |
| Skills | product resources shipped with the app | Discovery and activation are a runtime responsibility |
| Subagents | capability built on Pi extension interfaces | Define delegation, context, permissions, cancellation first |
| Agent memory | its own store behind the seam | Never a shared database as the integration point (D-008) |

Create a folder when the capability has code, tests and an owner — never in advance. Extensibility
vocabulary (Pi extension, included capability, skill, MCP integration, installable plugin) is defined
in the local `.docs/04-arquitectura-base.md`; treat it as provisional until versioned.

## Important flows

- **List workspaces:** `src/App.vue` `onMounted` → `listWorkspaces()` in `src/contracts/commands.ts` →
  `commands::list_workspaces` → `Store::list_workspaces` → SQLite.
- **Add workspace:** view trims input → `addWorkspace(path, name)` → command rejects a path that is
  not an existing directory → store insert (unique path) → view refreshes the list.
- **Settings round trip:** `getSettings()` / `saveSettings()` → `settings.rs` load/save whole with an
  atomic write; unknown fields are tolerated so newer files still parse.
- **Prompt with images:** the composer collects candidates (the `+` picker, a screenshot paste, or a
  native desktop drop) into `src/lib/composer-attachments.ts`, keyed by draft. Sending hands them to
  `sendPrompt(...)` → `commands::send_prompt`, which validates count, format and size, asks whether the
  active model declares image input (`get_state` of the live child, else pi's catalogue) and refuses
  with an explanation instead of switching models. The prompt line carries pi's own `ImageContent`;
  the transcript and the stored preview keep only the image count.
- **Session start (planned):** command → runtime supervisor spawns `pi --mode rpc` → events → typed
  Tauri events → Vue; `upsert_session` records the pi session path in the index.

## Accepted rules

- Presentation never owns security or infrastructure policy; the command layer validates.
- The database is a projection of pi's files, never a second source of truth for sessions.
- Contract shape changes start in Rust and are mirrored in TypeScript in the same change.
- Shared code needs identified consumers and regression checks before modification.
- Prefer deep modules with small interfaces; do not add a layer, bus or package per feature without a
  concrete need.

## Decisions

The decision register (D-001 … D-011, plus proposals T-001 … T-005) is local and gitignored in
`.docs/02-decisiones.md`. Durable decisions become ADRs in `docs/adr/`, which does not exist yet
(task A3). Until then, this file records only facts observed in source plus the rules above.
