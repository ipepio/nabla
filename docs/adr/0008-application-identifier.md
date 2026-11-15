# 0008 — Application identifier

- Status: Accepted (value provisional)
- Scope: Packaging / identity
- Acceptance evidence: `D-011` — identifier `com.ipepio.nabla`, product name `Nabla`, provisional until
  domain or trademark registration.
- Supersedes / superseded by: none

## Context

A Tauri application needs a reverse-DNS identifier and a product name before it can build. The
identifier is not a cosmetic label: Tauri derives the application data and configuration directories
from it, so it decides where `nabla.db` and `settings.json` live on each platform. Changing it later
moves that location and, in practice, orphans a user's existing state.

## Decision

Use `identifier: "com.ipepio.nabla"` and `productName: "Nabla"` in `src-tauri/tauri.conf.json`
(version `0.1.0`). `D-011` marks the identifier provisional pending domain/trademark registration.

Paths are never hardcoded from the identifier: `src-tauri/src/lib.rs` resolves directories once at
startup through `app.path().app_config_dir()` and `app.path().app_data_dir()`, so a later identifier
change is a configuration and migration problem, not a code rewrite.

## Alternatives

`com.nabla.app` or another domain-shaped identifier was not chosen: the maintainer's namespace is the
honest owner while the product name is unregistered. A name without a namespace (for example
`nabla.desktop`) is not reverse-DNS and risks collisions with other apps' directories.

## Consequences

- App data lives under the platform directory derived from `com.ipepio.nabla`; on macOS that is the
  identifier-named folder inside `~/Library/Application Support`.
- Changing the identifier after release requires a migration that moves `nabla.db` and `settings.json`
  and must ship with the change; this record's successor has to specify it.
- The window title (`Nabla`) is user-visible and comes from `tauri.conf.json`, not from a Rust
  constant.

## Conditions to revisit

- Domain or trademark registration for the product name: the successor record fixes the final
  identifier and describes the state migration for existing installs.
- Shipping a second environment (for example a beta channel) needs a distinct identifier, decided
  before that channel exists rather than at release time.
