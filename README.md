# Nabla

A desktop harness for coding agents built on [Pi](https://github.com/earendil-works/pi):
a Tauri v2 host in Rust, a Vue 3 + Tailwind webview, and one headless `pi` process per
session that the host owns, supervises and kills with the app.

Early, fast-moving, and honest about it: version 0.1.0, macOS only for now, no signed
builds.

## Requirements

| Tool | Version | Notes |
| --- | --- | --- |
| macOS | 13+ | the only platform exercised so far. Linux/Windows are untested |
| [Node.js](https://nodejs.org) | 22+ | Vite and the Tauri CLI |
| [Rust](https://rustup.rs) | 1.85+ | the crate uses edition 2024 |
| [pnpm](https://pnpm.io) | 12+ | the only package manager this repo uses |
| Xcode Command Line Tools | — | `xcode-select --install` |
| [Pi](https://github.com/earendil-works/pi) | latest | `npm install -g @earendil-works/pi-coding-agent` |
| [Poppler](https://poppler.freedesktop.org) (`pdftotext`) | latest | optional: attaching a PDF reads its text locally. Without it the composer tells you what to install (`brew install poppler`) |

Verify the last one, because Nabla resolves it by name from `PATH`:

```bash
pi --version
```

## Run it

```bash
corepack enable        # ships with Node 22+
git clone git@github.com:nabla-agent/nabla.git
cd nabla
pnpm install
pnpm tauri dev
```

`pnpm dev` starts only the frontend and takes no arguments (any argument, like
`pnpm dev tauri`, exits with a hint); the desktop app is `pnpm tauri dev`.

The first build compiles the whole Rust dependency tree; expect several minutes.
Later runs are fast. If `cargo` is not found, load the toolchain in that shell:
`source "$HOME/.cargo/env"`.

## First run: two things to configure

Nabla is a host, not a model. Without these two it starts but cannot hold a
conversation.

1. **A provider and its key** — open Settings and add a provider with its API key.
   Nabla writes the key to Pi's own model registry; keys never enter this
   repository and are not committed anywhere.
2. **A model** — pick the model new sessions start with. The composer shows it, and
   the effort selector next to it controls how much the model thinks.

If sessions fail to start with `failed to spawn pi (pi)`, the app could not find the
binary. Launching from a terminal (`pnpm tauri dev`) inherits your shell `PATH` and
usually fixes it; a Finder-launched app does not. As a last resort set the absolute
path in `settings.json`:

```json
{ "piBinary": "/Users/you/.nvm/versions/node/v22/bin/pi" }
```

## Where your data lives

`~/Library/Application Support/com.ipepio.nabla/`

| Path | What |
| --- | --- |
| `settings.json` | your preferences, human-editable |
| `nabla.db` | SQLite index of workspaces and sessions (metadata only) |
| `sessions/*.jsonl` | the transcripts, written by Pi. These are the source of truth |
| `agents.json` | subagent profiles (appears once you configure one) |
| `permissions.json` | the current permission mode, as a projection |

Conversation content lives only in Pi's JSONL files; the database is a rebuildable
index, never a second copy. To start from zero: quit Nabla and remove that folder.

## Turning capabilities on

A fresh install starts with everything optional **off**. That is the intended
baseline, not a missing feature:

| Capability | Default | Where |
| --- | --- | --- |
| Web search | off | Settings → Web search |
| Subagents | off (no `agents.json` until you configure one) | Settings → Subagents |
| Permission mode | `write` — edits allowed, commands ask | the chip in the composer |

The two features ship off because both leave your machine:

- **Web search** — Settings → Web search. A SearXNG instance you name, or any public
  one if you leave the field empty. The agent gets titles, urls and snippets; the page
  itself is never opened by the search. The switch gives the agent the tool; the
  composer's per-turn *web search* request stays disabled until the SearXNG round trip
  is verified end to end (see below).
- **Subagents** — Settings → Subagents. One master switch, then one switch per
  profile. With it off the agent has no delegation tools at all. Templates (explorer,
  researcher, worker, judge, judge-2) arrive written and disabled; enable the ones you
  want. Changes apply to the **next** session, not the running one.

The permission chip in the composer is the other switch worth knowing: `read` blocks
shell and file edits, `write` allows edits and asks before commands, `yolo` allows
everything.

**The composer's `+` menu** attaches an image or a PDF, or marks a per-turn tool
request. A PDF is extracted locally by `pdftotext`, page by page, and travels with
your question; page numbers survive pages that hold nothing, and a scan (no
selectable text) is reported instead of being sent, because no OCR runs here. A tool
request — web search, the embedded browser — is one line added to that turn's prompt:
it changes no setting, runs nothing by itself, and is only offered when the
capability is really on. Both requests need the run's own tool endpoint, which
publishes the browser tools and `web_search`; the browser request additionally needs
the Browser switch in Settings → Tools, because it drives a real Chromium instance.
Web search is **not** offered yet: a switch and a bound endpoint do not prove that a
SearXNG instance answers a query, so the row stays visible and disabled with
"End-to-end search verification pending" until that round trip has been verified.
Otherwise the row stays visible, disabled, with the reason.

## Checks

```bash
pnpm run build                        # frontend gate: vue-tsc --noEmit + vite build
node --test tests/                    # Node tests for the pure modules and composables
cd src-tauri && cargo test            # Rust tests
cd src-tauri && cargo clippy          # Rust lint
```

Vue components are still checked by hand (`pnpm tauri dev`): the Node suite covers
the pure modules, and the window-level states — a pasted screenshot, a dropped
file, a removable preview — are exercised there.

### Rust build cache and dev diagnostics

`src-tauri/target/` is a rebuildable Cargo cache, not the installed app or your
sessions. Check it with `du -sh src-tauri/target`. Desktop builds only produce the
Rust library the executable links, not mobile FFI libraries. Dev builds keep
file/line backtraces and incremental compilation, but omit full variable debug
information. For a debugger session that needs locals, use
`CARGO_PROFILE_DEV_DEBUG=2 pnpm tauri dev` (expect larger artifacts).

On one macOS checkout, a fresh `cargo build` used 1.4 GiB, or 2.2 GiB after tests
and Clippy. These are observations, not limits. Changing profiles does not remove
old artifacts. To reclaim the old cache, stop `pnpm tauri dev` and other Cargo
jobs, then run `(cd src-tauri && cargo clean)`; the next build will be cold.

For first-click queue problems or an embedded browser that opens blank, watch
the terminal running `pnpm tauri dev`. Debug traces report queue decisions and
native view creation/cleanup. A visible Browser tab alone does not mean its
native view is ready. These diagnostics are not a release logging feature.

## Build a desktop app

```bash
pnpm tauri build
```

The bundle lands in `src-tauri/target/release/bundle/`. It is **unsigned and
unnotarized**, so macOS will refuse it on first launch: right-click → Open, or
`xattr -dr com.apple.quarantine /Applications/Nabla.app`. Sharing a `.dmg` with testers
means they will each meet Gatekeeper; telling them the incantation in advance saves the
confusion.

## What to try, and what will bite you

Worth exercising: paste a screenshot into the composer (or drop one on it) and ask a
question about it; attach a PDF from the `+` menu and ask which page each conclusion
appears on; queue a second question while a turn runs and watch it keep its own
attachments; delegate to a subagent and watch its session appear nested under the
chat that started it; open that child and watch it work, read-only; stop the parent and
see the children die with it; ask the agent to drive the embedded browser pane.

Known for now: no screenshot tool for the embedded webview; a session's changes to the
subagent master switch need a fresh session; the app is macOS-only and has no
auto-update.

## Documentation

- [AGENTS.md](AGENTS.md) — the working contract: architecture map, non-negotiable rules
- [CONTENTS.md](CONTENTS.md) — the index of deep-dives under `docs/`
- [docs/adr/](docs/adr/) — architecture decisions, including the security baseline
- `odd/tasks/` — feature specifications and their logs

## License

Apache License 2.0 — see [LICENSE](LICENSE).
