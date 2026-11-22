# Component architecture — Vue layer

Rules for new or modified code in `src/`. The current layer is deliberately small: one shell view and
one contracts module. Do not add layers that the current screens do not need.

## Responsibility boundaries

| Kind | Example / path | Owns | Must not own |
| --- | --- | --- | --- |
| Shell view | `src/App.vue` | Screen composition and the user flow of the shell | Transport details, DTO mapping, process lifecycle |
| Domain component | `src/<domain>/components/` (when created) | One coherent interaction | Unrelated domains, hidden global side effects |
| Shared UI | shadcn-vue (`src/components/ui/*`, Tailwind v4) | Presentation and interaction contract | Domain state, command calls |
| Composable | `src/<domain>/` (when created) | A cohesive reactive responsibility and its cleanup | A grab bag of helpers |
| Contracts module | `src/contracts/commands.ts` | The only IPC channel and error normalization | Navigation, dialogs, form state |
| Contracts types | `src/contracts/types.ts` | Mirrors of Rust DTOs | Logic, defaults, derived values |

## The IPC channel

- `src/contracts/commands.ts` is the **only** place that imports `@tauri-apps/api/core` and calls
  `invoke()`. Components import functions from it and nothing else.
- `src/contracts/types.ts` mirrors the Rust structs field by field in camelCase. When a payload
  changes, the Rust DTO changes first and this file follows in the same change.
- Bridge rejections are plain strings today: always render them through `commandErrorMessage()` so
  the UI never shows `[object Object]`. Typed error payloads are a pending gap, not a rule.
- No component knows a command name as a string; each one has a typed wrapper.

## shadcn-vue and the shell

- The UI system is shadcn-vue on Tailwind CSS v4, dark theme as default (`.dark` on `<html>`;
  tokens in `src/assets/index.css`). Keep one UI system; do not add another component library or
  CSS framework.
- Generated shadcn-vue components live in `src/components/ui/*` and are owned like vendored code:
  edit them only with a reason, prefer composing them. `src/lib/utils.ts` exposes `cn()`.
- Build screens from the shell parts: `AppSidebar` (left drawer: logo, workspaces, sessions &
  nested subagents, settings trigger) + `SidebarInset` (central chat column + resizable right
  tools/inspector drawer `ResizableAside`). Global overlays (`SettingsDialog`, `CommandPalette`)
  are lazy-loaded on first open.
- Icons come from `@lucide/vue` (one icon family only); accessibility belongs to the component:
  every icon-only button carries an `aria-label`.
- Colors are semantic tokens, never literals: use the Tailwind utilities that map to them
  (`bg-background`, `text-muted-foreground`, `bg-primary`, `text-success`, `bg-tertiary`, `border-border`…).
  The values live in `.env` (`VITE_COLOR_*`) and reach the CSS variables through `src/lib/theme.ts`;
  a hex or `rgb()` in a component is a defect (ADR 0009, [conventions](conventions.md#theming-and-branding)).
- Brand images resolve through `brandAsset()` from `src/lib/brand.ts` (`mark`, `logo`, `wordmark` and
  their `*Light` siblings). A component never writes an asset path such as `/logo.png`; changing the
  image in `public/` re-brands the UI without touching code.
- CSP stays enabled; `style-src 'unsafe-inline'` is currently required by the Tailwind dev
  pipeline, remote assets are not (see ADR 0007).

## State ownership

- Local `ref` first. Use a store only when state must be shared or must survive a screen change; no
  store exists today, and adding one (Pinia or otherwise) needs a concrete need and approval.
- Keep one loading flag per operation (`workspacesLoading`, `adding`, `savingSettings`) so independent
  actions never block each other.
- Model absent, loading, failed and successful states explicitly. Errors are `string | null` rendered
  in an `Alert`; success is transient feedback, never a silent state change.
- Never mutate a prop or a caller-owned object; emit the intent to the owner instead.
- After a successful write, reload or patch the affected data deliberately — do not assume the
  database changed the UI state.

## Drawer startup — one chooser tab, one boot read

The tools drawer is the first screen a session reaches for, so its startup path is one open and one
settings read (S1/S2 of `odd/tasks/drawer-responsiveness-oauth.md`):

- **The drawer opens on its chooser.** `openDrawer()` in `src/components/tools/dock.ts` is the only
  path that seeds a tab: it reconciles the dock with the active workspace and the enabled tools
  first, then adds one EMPTY tab when the drawer has none, so choosing a tool is one click instead
  of `+` and then a pick. `App.vue` calls it when the human opens the drawer. Seeding is tied to the
  open and never to the reconcile: closing the last tab while the drawer is on screen leaves it
  empty (a later workspace or settings reconcile does not resurrect it), and reopening keeps every
  tab, its panes and the active tab exactly as they were. The agent's `openTool()` fills the empty
  active tab instead of leaving it beside a new one, and still reveals an existing app-wide pane
  (Browser) rather than duplicating it.
- **Heavy non-startup surfaces are lazily mounted.** `TerminalTool` (`src/components/tools/registry.ts`),
  `SettingsDialog` (`AppSidebar.vue`) plus its sub-sections (`McpSection`, `SubagentsSection`,
  `ProviderModelsEditor`, `AccountsSection`, `ProviderForm`), `CommandPalette` (`App.vue`), and
  `SubagentView` (`ChatPanel.vue`) are wrapped in `defineAsyncComponent(() => import(...))` so
  their code is only parsed and evaluated on first open. Files, Changes and Browser stay eager
  because Files/Changes are opened immediately and Browser coordinates its child webview with
  the drawer's first paint.
- **CSS containment keeps long transcripts and drawer resizes isolated.** Settled turns in
  `TurnGroup.vue` apply `content-visibility: auto` with `contain-intrinsic-block-size: auto 12rem`,
  and `ResizableAside.vue` applies `contain: layout style`. Entry and disclosure transitions
  (`src/assets/index.css`: `.nabla-enter`, `.nabla-disclosure`, `.nabla-status-pulse`) animate
  only compositor properties (`opacity`, `translate`, `transform`) and honour
  `prefers-reduced-motion`.
- **The boot settings read is handed over once.** `main.ts` already reads `settings.json` before the
  first mount for the language; it hands that payload to `src/lib/boot-settings.ts`
  (`rememberBootSettings`), and the chat's startup `loadSettings()` takes it (`takeBootSettings`)
  instead of issuing a second identical command. Taking is one-shot: every later read — the settings
  dialog, the read-merge-save of a theme or model write — asks the command again, so a hand-edited or
  externally written file is never answered from a payload read before mount.

## Drawer refresh — one workspace, one scan, one snapshot

A turn landing anywhere used to move one global counter, and the drawer answered it by dropping every
cached list and refetching every open workspace (S2 of `odd/tasks/drawer-responsiveness-oauth.md`).
Three rules now keep that cost proportional to what actually changed, and each one lives where it can
be stated once:

- **The refresh is workspace-scoped.** `src/lib/session-refresh.ts` holds the planner (`sessionRefresh`,
  created by `createSessionRefreshPlanner`). The store calls `note(workspaceId)` where activity
  originates — a prompt sent, a turn finished, a queued prompt sent, a session created — and the drawer
  watches `signal` and asks `takePlan(expandedIds)` which open lists to re-read. Only an expanded
  workspace whose revision moved is re-read: a change in a closed list costs nothing (expanding reads
  it anyway), and a change the store cannot attribute to a workspace (an event for a session it never
  placed) asks for every open list. Rows are preserved: the refresh goes through the existing
  `loadSessions`, which replaces `sessionsByWorkspace[workspaceId]` only when a different index lands
  and still lets the newest request win, so nothing blanks the drawer while a re-read is in flight.
  `sessionGeneration` keeps its own meaning (a session was created, the selection was dropped) and is
  watched by the same handler.
- **A snapshot is written once per tail.** `turn_end` and `agent_end` both close the same turn and both
  snapshot the session preview. `savePreviewOnce(sessionId, tail, write)` fingerprints the payload
  (role, text, reasoning and time — reasoning is what pi's file never stored) and skips a tail it
  already stored, so the duplicate completion costs no serialization and no row write. The semantics
  are unchanged: every turn that moved the conversation is written, a write that answered `false` or
  rejected is not remembered (the next completion retries it), and `forgetSavedPreview` drops the
  memory when the session is deleted.
- **One manifest scan per query.** `subagent_sessions_for` reads the extension's `<data dir>/subagents/`
  directory once and passes that snapshot to `sync_subagent_children`, which uses it for the rows it
  indexes *and* for the live status the answer carries. The directory is never walked twice for one
  listing; the scan counter behind that claim is test-only and per thread.

The two frontend rules are pure and covered in `tests/session-refresh.test.ts`; the scan is covered by
`subagent_sessions_scans_the_manifests_once_per_query` in `src-tauri/src/commands.rs`.

## Sidebar session rows — active children inline, finished children folded

A parent's subagent rows are derived from the status the listing returned, so no component stores group
membership (`src/components/sidebar/SessionsList.vue`):

- **Active children stay inline.** `activeChildrenOf` keeps the rows whose status is running or queued
directly under their parent, with the stop button on hover and in the context menu, because the child an
agent is working in is never behind a disclosure.
- **Finished children fold into one row.** `finishedChildrenOf` collects everything else behind a single
disclosure per parent — `chat.subagent.finishedOne` / `chat.subagent.finishedMany` in `src/locales/` —
closed by default, with a chevron and `aria-expanded`. The expanded set (`expandedFinishedGroups`) is
keyed by session id and lives in component memory only. A child that finishes moves from the inline list
into the group on the next listing, since membership is computed and never written.
- **The tail is removable in one action.** The group row carries a trash button on hover and its own
context-menu variant (`finishedGroup`) offering show/hide plus
`chat.subagent.deleteAllFinishedCount`. `handleDeleteAllFinished` closes the read-only subagent view
first when the open child is inside the set, deletes with `Promise.allSettled` so one failure does not
strand the rest, forgets the group's expanded state, and force-reloads that parent's children only —
never the whole drawer.

## The Browser pane — one pane, N tabs

A page that opens a popup gets a tab **inside the Browser pane** — not a drawer tab and not a window of
its own (S3/L3 of `odd/tasks/drawer-responsiveness-oauth.md`; the native story and its limits are in
[browser-popups.md](browser-popups.md)). The pane therefore carries a small tab strip, and the rules
that make it safe to have a live auth window sitting next to the agent's page are split in two:

- **`src/components/tools/browser-tabs.ts` owns the model and is pure.** One tab per child webview, a
  popup inserted right after the view that opened it, exactly one drawn tab, a new tab taking the
  focus, a manual close returning to the opener, and no path at all that closes the page tab (it *is*
  the pane). A mutation that changes nothing returns the state it was given, so a caller can compare
  by identity, and a close answers the ids the pane must close natively. Pinned by
  `tests/browser-tabs.test.ts`.
- **`BrowserTool.vue` owns the platform.** Every tab is one child webview: the page is the one the pane
  created (`embed_create`, the view the agent drives), a popup arrives already built — Rust reparented
  it into the main window and keeps it hidden — and is adopted from `embed-popup`, or from
  `embed_popups` when the event fired while no listener was up. Only the drawn tab's view is placed and
  shown; the others are hidden with `embed_set_visible` and keep their document, history and login.
  Closing the page closes its popups: in the model, and in Rust, which cascades by opener and closes by
  label so a view the registry already dropped is still reached.
- **`BrowserTabs.vue` is presentation only**: it draws the items `stripItems` returns and emits
  `select`/`close`. It appears with the first popup, because a strip with one tab is noise.
- **What the agent can reach does not change.** The page tab stays the target; a popup is excluded from
  the answers the MCP bridge reads (`ready_views`, `is_ready`) because it carries the human's live
  provider session. The pane's footer still describes the page, never the popups.
- **The missing native signal is documented, not hidden.** macOS never reports `window.close()`, so a
  popup tab's close button is the end of its life — stated in [browser-popups.md](browser-popups.md) and
  in the component's own notes, instead of pretending the tab ends itself.

## Composer images — three seams

Attaching, previewing and sending an image (`odd/tasks/composer-image-input.md`, S1-S2) is split the
same way, so no layer guesses at another's rule:

| Layer | Owns | Contract |
| --- | --- | --- |
| `src/components/chat/composer-images.ts` | What the composer accepts | The four formats pi sends inline (`image/png`, `image/jpeg`, `image/gif`, `image/webp`); `COMPOSER_IMAGE_LIMIT` images per turn and `COMPOSER_IMAGE_MAX_BYTES` per image; base64 shape and decoded size; *a clipboard with text pastes its text*, only an image-only clipboard becomes attachments; the label a transcript keeps (never the payload). Pinned by `tests/composer-images.test.ts`. |
| `src/lib/composer-attachments.ts` | Where a pending image belongs | One list per draft key (`nabla.draft.<sessionId>` or `nabla.draft.new.<workspaceId>`), in memory only: switching conversations switches images, a rejected send keeps them with the text (`moveAttachments` covers the key the first send creates), and nothing ever writes base64 to `localStorage`, a log line or SQLite. |
| `src-tauri/src/image_input.rs` | What a file and a prompt may carry | The format is sniffed from the bytes, never the extension; the metadata size is checked before the file is read; at most `MAX_PROMPT_IMAGES` per prompt and `MAX_IMAGE_BYTES` per image; the payload leaves as pi's own `ImageContent` (`type: "image"`, `data`, `mimeType`). `send_prompt` runs the same bounds again, so the webview is never the authority. |

`Composer.vue` only wires the entry points: the `+` menu asks the system pickers (through
`src/contracts/`), a paste is filtered by the clipboard rule, and a desktop drop arrives from the
window because Tauri hands the OS paths there, not to the DOM (`onFileDrop`, positioned against the
card's rectangle). `ChatPanel.vue` turns every candidate into a pending attachment and sends or queues
the whole turn (text plus images, or images alone) through the same `sendUserTurn` / `enqueuePrompt`
path a text prompt uses.

## Composer PDFs and tool requests — four seams

The same feature (`odd/tasks/composer-tools-pdf.md`, S2-S3) adds two things a turn can carry besides
its text, and each rule lives in exactly one module:

| Layer | Owns | Contract |
| --- | --- | --- |
| `src-tauri/src/pdf_input.rs` | Extraction and its bounds | `pdftotext` runs as an argv-only child (never a shell string); the file must be a regular file under 32 MiB starting with `%PDF-`; the child is killed at 20 s and its stdout is capped; the answer is refused over 200 pages or 1,000,000 characters. The form feed splits pages, so page N is index N-1 and an empty page stays `""`; a document with no selectable text answers `hasText: false` and no OCR runs. A missing `pdftotext` names the poppler package. Pinned by the module's own `#[cfg(test)]` tests, which build their PDFs in a temp dir. |
| `src/components/chat/composer-pdfs.ts` | What the composer accepts | `COMPOSER_PDF_LIMIT` documents per turn; the page/character ceilings re-applied to the payload; the page-numbered block (`[page N]` before each page, nothing for an empty one); `hasUnreadablePdf` for the scan case, which attaches and blocks sending. Pinned by `tests/composer-pdfs.test.ts`. |
| `src/components/chat/composer-tools.ts` | What a tool request is | Text only: `web-search` and `browser`, each one explicit line the turn carries. Both are published from the local MCP endpoint this run bound and registered with pi, so without it neither request exists — whatever `settings.json` says. Above that each has its own human switch: `browser` needs the Browser tool in Settings → Tools (`enabledTools`), and `web-search` needs `settings.webSearchEnabled`. `web-search` additionally stays disabled with `search-verification-pending` until the SearXNG round trip is proven end to end: a switch and a bound endpoint are not that proof, and `webSearchEndToEndVerified` is the single place that fact changes. A request that is not available stays visible, disabled, with the reason — the first unmet precondition, in the order the rule reads it — never advertised as ready. Choosing one executes nothing, flips no setting, makes no network call, and the underlying service is never disabled. Pinned by `tests/composer-tools.test.ts`. |
| `src/components/chat/composer-turn.ts` | What a turn really contains | `composeTurnPrompt` is the single place question + request lines + PDF blocks become the wire text, and `composeTurnDisplay` the single place the transcript/queue label is built. The panel composes once per send or queue, then clears the draft; the label names attachments, never their content. Pinned by both Node suites. |

`ChatPanel.vue` owns the facts a row is decided from: the MCP endpoint is read once at mount (a
process property, not a turn's), `webSearchEnabled` and `isToolEnabled("browser")` come from the
shared preference refs, so a flip in the settings dialog reaches an open menu without a reload, and
no switch is ever inferred from the endpoint being up.

The queue owns what it is given: `enqueuePrompt` / `sendUserTurn` store the composed text, the image
payloads and the label, so a queued turn keeps its PDFs and its requests while it waits, and a
rejected send leaves the whole draft (text, images, PDFs, requests) in place. Nothing extracted is
ever persisted: the PDF text lives in `src/lib/composer-attachments.ts` for as long as the draft does.

**The capability preflight is two-layered on purpose.** The composer shows the notice as soon as the
named model's catalogue entry lacks `image` in its `input` list, and Rust refuses before writing to the
child when the *live* model's own `get_state` says the same (falling back to the catalogue when pi
reports no model). Nothing switches models automatically: the human picks, and a refused turn keeps
the draft.

## Long conversations — three seams

A conversation that has run for hours is a long list of turns, a growing answer and a scroll column that
receives requests from everywhere at once. Three pure modules keep that bounded, and each one states its
contract so a view never re-implements it:

| Module | Owns | Contract |
| --- | --- | --- |
| `src/components/chat/turn-window.ts` | How many turn groups are mounted | The newest page (`INITIAL_VISIBLE_TURNS`, default 12) plus one page per `showOlder()`; it is derived from the list, so history that arrives *after* mount is bounded too; the visible window ends at the newest turn, so the live turn is always mounted; a change of identity (session id, open child) starts over. |
| `src/components/chat/code-segments.ts` (`createSegmentCache`) | Reuse of already-formatted blocks | `segmentsFor(text)` returns exactly what `segmentsOf(text)` returns, always. It may only reuse when the new text extends the previous one, and only up to the last *complete* line where the parser would start a fresh block — a heading, a list row that does not continue the run above it, prose after a non-prose line, and the closing line of a fence. An open paragraph or an open list run is never split, so no block boundary is invented. The search for those boundaries is incremental: a delta reads only the lines that arrived since the previous call, so a plain formatted answer does not pay a full scan per delta. A replaced, truncated or reordered text restarts the parse. |
| `src/components/chat/transcript-follow.ts` (`createTranscriptFollow`) | Every movement of a transcript's scroll offset | `request()` scrolls at most once per frame and only while the view is pinned to the end; moving up unpins (`handleScroll`), returning re-pins, `handleResize` re-glues immediately, `detach()` cancels the pending frame, and `snapshot()`/`restore()` keep a reader's place when content is added above them. |

Both transcripts (`ChatPanel.vue`, `SubagentView.vue`) wire the same three: a turn window over their
groups, a follow controller over their scroller, and `OlderTurnsControl` above the first mounted turn,
whose reveal is anchored with `snapshot()`/`restore()` so the reader keeps the line they were on. The
views keep ownership of their refs, listeners and `ResizeObserver`; the modules keep the policy.

## Tests

- Deterministic behaviour of the pure modules and composables lives in `tests/*.ts` and runs with the
  runtime already on the machine: `node --test tests/` (Node strips the types; no test framework is a
  dependency). The files import the modules with their `.ts` extension, which `tsconfig.json` allows
  (`allowImportingTsExtensions`).
- Test only through the public contract, and compare against the naive reference where one exists (the
  segment cache is checked against `segmentsOf`, prefix by prefix and on generated append-only streams;
  reuse itself is observed as block identity, because that is what a caller can see).
- A module that imports `.vue` files or the `@/` alias is still testable: `tests/drawer-startup.test.ts`
  registers Node's synchronous module hooks (`registerHooks`) to map the alias, stub `.vue` modules
  and resolve extensionless `.ts` imports, then imports the real `dock.ts` and asserts the tab model
  through its public functions. The same hook records which `.vue` modules were evaluated, which is
  how the deferred Terminal load is proven from the outside.
- A test that needs state on disk creates it in a fresh `mktemp -d` and removes it before it ends; no
  scratch path, cache or fixture is left in the worktree.

## Extraction

- Extract when a block is a coherent interaction with clear inputs and outputs, not because a file
  grew long. A 300-line shell view with two panels is acceptable; two files with one panel each whose
  contracts are unclear is not.
- Promote to shared UI only when the contract is independent of the domain and a real reuse case
  exists. A wrapper must add policy; do not add pass-through components for symmetry.
- Keep pure transformations as plain functions; use `computed` for derived values and `watch` only
  for effects.

## Review checklist

Identify each changed component's category, state owner, inputs/outputs and the UI states it must
handle (see [verification](verification.md)). Check keyboard operation, labels and focus when you
touch an interactive control. `npm run build` proves types and bundling, not that the screen works.
