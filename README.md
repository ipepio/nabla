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

Two features ship **off** and are enabled deliberately, because both leave your
machine:

- **Web search** — Settings → Web search. A SearXNG instance you name, or any public
  one if you leave the field empty. The agent gets titles, urls and snippets; the page
  itself is never opened by the search.
- **Subagents** — Settings → Subagents. One master switch, then one switch per
  profile. With it off the agent has no delegation tools at all. Templates (explorer,
  researcher, worker, judge, judge-2) arrive written and disabled; enable the ones you
  want. Changes apply to the **next** session, not the running one.

The permission chip in the composer is the other switch worth knowing: `read` blocks
shell and file edits, `write` allows edits and asks before commands, `yolo` allows
everything.

## Checks

```bash
pnpm run build                        # frontend gate: vue-tsc --noEmit + vite build
cd src-tauri && cargo test            # Rust tests
cd src-tauri && cargo clippy          # Rust lint
```

There is no frontend test runner; UI behaviour is verified by hand.

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

Worth exercising: delegate to a subagent and watch its session appear nested under the
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
