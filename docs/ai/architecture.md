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
| PDF input | `src-tauri/src/pdf_input.rs` | Local `pdftotext` extraction, page numbering and its bounds (file, pages, characters, time) | OCR, inventing text for a scan, persisting an extraction |
| Runtime | not created | Spawning/stopping pi sessions, streaming events | Depending on Vue or presentation details |
| Zoom shortcuts | `src/lib/zoom.ts`, `src/contracts/window.ts`, `src-tauri/src/zoom.rs` | The chord, the step, the range, and which port receives the factor | DOM or Tauri calls outside the ports it is handed |

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
- **Prompt with images:** the composer collects candidates (the `+` menu's image entry, a screenshot
  paste, or a native desktop drop) into `src/lib/composer-attachments.ts`, keyed by draft. Sending
  hands them to `sendPrompt(...)` → `commands::send_prompt`, which validates count, format and size,
  asks whether the active model declares image input (`get_state` of the live child, else pi's
  catalogue) and refuses with an explanation instead of switching models. The prompt line carries
  pi's own `ImageContent`; the transcript and the stored preview keep only the image count.
- **Prompt with a PDF and/or a tool request:** the same draft holds the PDFs and the per-turn tool
  requests. The PDF entry (or a dropped `*.pdf`) goes to `read_pdf_attachment` → `pdf_input.rs`, which
  runs `pdftotext` as an argv-only child and answers the selectable text of every page, bounded in
  size, pages, characters and time, keeping page numbers even for empty pages and reporting a scan as
  `hasText: false`. `src/components/chat/composer-turn.ts` composes the turn's text once — question,
  one line per requested tool, then the page-numbered document — and the label the transcript and the
  queue keep names the attachments without copying the text. A request is a line of the prompt, never
  an execution, a settings write or a permission bypass.
- **Session start (planned):** command → runtime supervisor spawns `pi --mode rpc` → events → typed
  Tauri events → Vue; `upsert_session` records the pi session path in the index.
- **Zoom (`⌘`/`Ctrl` `+`, `-`, `0`):** a document-level `keydown` in `src/App.vue` → `zoomAction` /
  `nextZoomLevel` in `src/lib/zoom.ts` (pure; covered by `tests/zoom.test.ts`) → `setWebviewZoom()` in
  `src/contracts/window.ts` → `commands::set_webview_zoom` → `zoom::validate_scale` → Tauri
  `Webview::set_zoom` on the **main** webview. That is `wry`'s page zoom: `WKWebView.setPageZoom`
  (macOS 11+), WebView2 `SetZoomFactor`, webkit2gtk `set_zoom_level` — present in this build on all
  three desktops, taking a factor where `1.0` is 100%. Zoom belongs to ONE webview, so the embedded
  browser's child webviews (`embed-<n>`) keep their own scale, and because page zoom does not change
  CSS pixels, the rect the UI sends for a child stays valid under zoom. The JS route
  (`getCurrentWebview().setZoom()`) exists but is a `core:` command and would need
  `core:webview:allow-set-webview-zoom` in `capabilities/default.json`; the app's own command answers
  the same question without widening a capability for one keystroke. A refusal (a plain browser during
  `vite dev`, or a platform without it) falls back to CSS `zoom` on the root element, which is a
  DEGRADED mode: viewport units and the child-webview rect stop agreeing — that is why native is
  always tried first and why the fallback is sticky.

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
