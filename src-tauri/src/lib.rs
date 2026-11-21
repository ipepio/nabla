mod agents;
mod cli;
mod commands;
// The embedded browser: one child webview per view, added to the main window.
mod embed;
mod mcp;
// The other end of MCP: a local server pi connects to in order to drive the
// embedded webview (`webview-control.md`).
mod mcp_server;
mod process;
mod providers;
mod rpc;
mod settings;
mod shell;
mod store;
// `web_search`: the agent's web search, backed by a SearXNG instance the human
// configures or, failing that, by any public one (`web-search.md`).
mod websearch;
mod workspace;

use std::sync::Arc;

use tauri::{Emitter, Manager};

use commands::{AgentEventEnvelope, AppState};
use process::ProcessManager;
use rpc::AgentEvent;
use store::Store;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // Native folder picker for the workspace add flow (S9).
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Directories are resolved once at startup: Tauri maps them to the
            // platform-specific locations, so no path is hardcoded here.
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            // SQLite creates the file but not its parent directory.
            std::fs::create_dir_all(&data_dir)?;
            // S1/S2 of `agent-question-bridge.md`: the first-party question
            // extension is written from the source embedded in this binary, so
            // `-e` always points at a file that matches this build. Not fatal:
            // without it sessions run the same, only without the three ask tools.
            let ask_extension = match process::materialize_ask_extension(&data_dir) {
                Ok(path) => Some(path),
                Err(err) => {
                    eprintln!("could not materialize the question extension: {err}");
                    None
                }
            };
            // M2 of `permission-modes.md`: the permission gate is written the same
            // way, from the source embedded in this binary. Not fatal either, but
            // this is the one failure that must be loud: without the gate every
            // tool call of every session runs UNGATED (no blocking, no asking).
            let permissions_extension = match process::materialize_permissions_extension(&data_dir)
            {
                Ok(path) => Some(path),
                Err(err) => {
                    eprintln!(
                        "could not materialize the permission gate: {err} — every session's \
                         tools will run WITHOUT any permission check"
                    );
                    None
                }
            };
            // H2 of `subagent-helpers.md`: the delegation helpers are written the
            // same way. Not fatal: without them a session simply cannot delegate.
            // S3 of `agent-profiles.md`: writing them here does not load them —
            // every spawn asks `agents.json` whether the human opened the gate, so
            // the switch only has to be on when the NEXT session starts.
            let subagents_extension = match process::materialize_subagents_extension(&data_dir) {
                Ok(path) => Some(path),
                Err(err) => {
                    eprintln!("could not materialize the subagent helpers: {err}");
                    None
                }
            };
            // L4 of `permission-modes.md`: the gate reads the current mode from
            // `<data_dir>/permissions.json` on every tool call, so the projection
            // has to exist before the first child does. A failure is not fatal —
            // the gate falls back to `write` when it cannot read — but it is said.
            if let Err(err) = commands::sync_permission_projection(&config_dir, &data_dir) {
                eprintln!("could not write the permission projection: {err}");
            }
            let store = Store::open(&data_dir.join("nabla.db"))?;
            // S9: the first launch seeds the user's home directory as the default
            // workspace. `app.path()` is Tauri's resolver; `std::env::home_dir`
            // is deprecated and platform-dependent.
            let home = app.path().home_dir()?;
            commands::seed_default_workspace(&store, &home)?;
            // One supervisor for every pi child (S3). Its sink is the only
            // bridge from a pi process to the webview (S6): each event leaves
            // as `agent-event` with the camelCase `{ sessionId, event }`
            // envelope the TS contracts mirror (R4).
            let handle = app.handle().clone();
            let runtime = ProcessManager::new(Arc::new(move |session_id: &str, event: &AgentEvent| {
                // The on-demand command probe is a child, not a session (its
                // runtime key carries the reserved `probe:` prefix). Its
                // lifecycle frames would reach the store as an unknown id and
                // land in the drawer's live list, so they stop here.
                if commands::is_probe_key(session_id) {
                    return;
                }
                let envelope = AgentEventEnvelope {
                    session_id: session_id.to_string(),
                    event: event.clone(),
                };
                let _ = handle.emit("agent-event", envelope);
            }));
            // Every child this app spawns is loaded with that extension (S2).
            let runtime = match ask_extension {
                Some(path) => runtime.with_ask_extension(path),
                None => runtime,
            };
            // ... and with the permission gate, plus the absolute data dir it
            // reads its projection from (M2, L4). The MODE is never forwarded:
            // an environment is fixed at spawn time, so it could not change
            // without restarting the session.
            let runtime = match permissions_extension {
                Some(path) => runtime.with_permissions_extension(path),
                None => runtime,
            };
            // H2 of `subagent-helpers.md`: and with the delegation helpers, so a
            // chat can start child sessions (their JSONL files land under the data
            // dir the host already owns).
            let runtime = match subagents_extension {
                Some(path) => runtime.with_subagents_extension(path),
                None => runtime,
            };
            let runtime = runtime.with_data_dir(data_dir.clone());
            // S1/S3 of `pi-cli-bridge.md`: the interactive CLI sessions emit
            // their `{ cliSessionId, stream, line?, exitCode? }` payload on
            // `pi-cli`, the same one-way bridge the runtime uses.
            let cli_handle = app.handle().clone();
            let cli_sink: cli::CliSink = Arc::new(move |event: &cli::CliEvent| {
                let _ = cli_handle.emit("pi-cli", event.clone());
            });
            // The embedded browser's registry, shared by the commands and by the
            // MCP server: the agent drives exactly the views the human sees.
            let embeds = embed::EmbedSessions::new();
            // `webview-control.md` T1: the MCP server pi connects to in order to
            // drive those views. A failure to bind is not fatal — the app runs
            // without agent control of the browser, which is the honest
            // degradation for a local port a firewall may refuse.
            let mcp_endpoint = mcp_server::start(
                Arc::new(mcp_server::EmbedDriver::new(
                    app.handle().clone(),
                    embeds.clone(),
                )),
                // S1 of `web-search.md`: the `web_search` tool reads its switch and
                // its instance from `settings.json` on every call, so a change in
                // the dialog reaches the next tool call instead of the next launch.
                Arc::new(websearch::SettingsSearch::new(&config_dir)),
                mcp_server::new_token(),
            )
            .ok();
            // `webview-control.md` S4, defect L8: whether this run's endpoint
            // reached pi's `mcp.json`. A session start waits on this gate before
            // spawning pi, because pi reads that file exactly once — a spawn
            // inside the registration's `remove`→`add` window would inherit a
            // config with no `nabla-browser` tools for its whole life.
            let mcp_registration = Arc::new(mcp_server::RegistrationGate::new());
            app.manage(AppState {
                store,
                config_dir,
                data_dir,
                // S3 of `providers-pane.md`: providers live in pi's own
                // `~/.pi/agent/models.json`, so the module needs the home dir
                // Tauri just resolved — not a second, invented location.
                home_dir: home,
                runtime,
                cli: cli::CliSessions::new(),
                cli_sink,
                // S2 of `tool-drawer-and-mcp.md` (F2): the workspace terminals
                // emit their `{ sessionId, stream, data?, exitCode? }` payload
                // on `shell-output`, a channel of their own so a terminal's
                // bytes never interleave with the agent's events.
                shells: shell::ShellSessions::new(),
                shell_sink: {
                    let shell_handle = app.handle().clone();
                    Arc::new(move |event: &shell::ShellOutputEvent| {
                        let _ = shell_handle.emit("shell-output", event.clone());
                    })
                },
                // The embedded browser's registry: ids and the last url each
                // child webview reported. Tauri owns the webviews themselves —
                // here the app only keeps the metadata it can test without a
                // window.
                embeds,
                mcp_endpoint,
                mcp_registration: mcp_registration.clone(),
                // One in-flight command probe per workspace, for the `/` list
                // before any session exists.
                probes: Arc::new(commands::CommandProbes::new()),
            });
            // S4 of `webview-control.md` (T3): the endpoint is written into pi's
            // global `mcp.json` so every session pi spawns already carries the
            // `nabla-browser` tools. On a background thread on purpose: the
            // registration spawns the `pi` CLI, and neither a slow nor a missing
            // binary may delay the window from painting. Best effort from here on:
            // without the entry the app works the same, only without agent control
            // of the embedded browser.
            //
            // Defect L8: the outcome is not swallowed any more. Every path —
            // registered, skipped, failed — completes the gate, so a session start
            // can only ever pay its bounded wait instead of hanging on a thread
            // that died silently. A failure is announced once, by the first
            // session start that has to live with it.
            let registration = app.handle().clone();
            let gate = mcp_registration.clone();
            let thread = std::thread::Builder::new()
                .name("nabla-mcp-register".to_string())
                .spawn(move || {
                    let state = registration.state::<AppState>();
                    gate.complete(commands::register_mcp_server(&state));
                });
            if thread.is_err() {
                // No thread, no report: the gate is opened with the failure so
                // every session start fails fast instead of waiting out the
                // timeout it would otherwise never learn the reason for.
                mcp_registration.complete(mcp_server::RegistrationOutcome::Failed(
                    "the registration thread could not start".to_string(),
                ));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_workspaces,
            commands::add_workspace,
            commands::remove_workspace,
            // S8: pin and recency support for the spaces sidebar.
            commands::set_workspace_pinned,
            commands::touch_workspace,
            // S7: the model selector's source of truth.
            commands::list_models,
            // S1-S4 of `providers-pane.md`: pi's providers, the custom provider
            // form and its native model discovery.
            commands::list_providers,
            commands::save_provider,
            commands::discover_models,
            // S2/S4: refreshing an existing provider's models from pi's file, so
            // the editor never needs the api key in the webview.
            commands::discover_provider_models,
            // S3/S4: removing a provider and setting its api key in pi's file,
            // with the same backup and atomic write as `save_provider`.
            commands::delete_provider,
            commands::set_provider_key,
            // S2/S3 of `models-by-provider.md`: the models of each provider and
            // the per-model editor, over the same file and the same backup and
            // atomic write. The listing is the union of `models.json` and the
            // read-only catalog pi caches in `models-store.json`.
            commands::list_provider_models,
            commands::update_provider_model,
            commands::remove_provider_model,
            // S2/S3 of `model-visibility.md`: hiding a model is Nabla's own
            // setting, never an edit of pi's read-only catalogues.
            commands::set_model_hidden,
            commands::list_sessions,
            commands::get_settings,
            commands::save_settings,
            // S1/S3/S6 of `agent-profiles.md`: the subagent profiles are Nabla's
            // own file, and the spawn reads it fresh to decide whether a session
            // can delegate at all.
            commands::get_agents_config,
            commands::save_agents_config,
            // S1/S2 of `permission-modes.md`: the chat header's chip reads and
            // writes the mode, and a write reaches a live session through the
            // projection file the loaded gate reads on every tool call.
            commands::get_permission_mode,
            commands::set_permission_mode,
            // S2 of `settings-dialog.md`: the dialog shows the preferences path
            // and opens that file with the system editor.
            commands::config_file_path,
            commands::open_config_file,
            commands::start_session,
            // S1/S3/S5: existing sessions are opened lazily, reused when live,
            // and read back from the pi-owned file.
            commands::open_session,
            commands::session_transcript,
            // H3 of `subagent-helpers.md`: the subagent children a chat's manifest
            // links to it, so the sidebar can nest them under the turn that
            // started them.
            commands::subagent_sessions,
            // The preview: the newest turns as the app last streamed them, with
            // their reasoning, so a session paints before pi boots.
            commands::save_session_preview,
            commands::load_session_preview,
            commands::live_session_ids,
            commands::send_prompt,
            // S2/S3 of `composer-inputs.md`: the effort selector and the `/`
            // completion. The level is applied live and kept as the next
            // session's default; the commands are pi's own list, proxied.
            commands::session_thinking_levels,
            commands::set_session_thinking_level,
            // S2 of `models-by-provider.md`: the composer's model pick, applied
            // to the live session (the persisted default is the webview's own
            // write, so the two can never disagree about the file).
            commands::set_session_model,
            commands::session_commands,
            // The `/` list before a session exists: a short-lived pi that answers
            // `get_commands` and is killed, so no snapshot has to be stored.
            commands::workspace_commands,
            // S3 of `agent-question-bridge.md`: the human's answer to a question
            // the agent asked. It writes the protocol line to that session's
            // child, which is the only channel an `extension_ui_request` has.
            commands::answer_ui_request,
            commands::stop_session,
            // Per-session actions: pause is the existing stop, delete archives
            // the transcript and removes the row.
            commands::delete_session,
            commands::session_status,
            // S1-S4 of `pi-cli-bridge.md`: the allowlisted bridge to the `pi`
            // command line, its interactive sessions and the typed account
            // state. Every call runs argv-only, never through a shell.
            commands::pi_cli_run,
            commands::pi_cli_start,
            commands::pi_cli_write,
            commands::pi_cli_stop,
            // S2 of `providers-pane-refinements.md`: the account state of many
            // providers in one call, probed with a bounded pool.
            commands::pi_auth_check_many,
            // S5: the same state as the last probe left it, read from nabla.db
            // so the pane paints its badges without spawning the CLI.
            commands::cached_provider_statuses,
            commands::pi_auth_check,
            // S1 of `tool-drawer-and-mcp.md` (F0): MCP servers configured through
            // pi's own CLI — listing, add, remove, sign-in and sign-out. The
            // listing is `pi mcp list --json`, never a file read, and the
            // sign-in reuses the interactive `pi-cli` session above.
            commands::mcp_list,
            commands::mcp_add,
            commands::mcp_remove,
            commands::mcp_logout,
            commands::mcp_file_path,
            commands::mcp_login_start,
            // F1 of `tool-drawer-and-mcp.md`: the Files and Changes tabs. Every
            // call is read-only and confined to the workspace it names.
            commands::fs_list_dir,
            commands::fs_read_preview,
            // Raw bytes, so an image preview never pays for base64.
            commands::fs_read_image_bytes,
            commands::git_status,
            commands::git_diff,
            // F2 of `tool-drawer-and-mcp.md`: the workspace terminal, one pty per
            // tab, streaming on the `shell-output` channel. Opt-in through
            // `enabledTools` and never exposed to the agent.
            commands::shell_open,
            commands::shell_write,
            commands::shell_resize,
            commands::shell_close,
            // The embedded browser: a child webview of the main window, moved
            // and sized by the UI, with its own lifetime per view. A real
            // webview, not an iframe (X-Frame-Options forbids that).
            commands::embed_create,
            commands::embed_set_bounds,
            commands::embed_set_visible,
            commands::embed_navigate,
            commands::embed_eval,
            commands::embed_close,
            commands::embed_url,
            commands::embed_close_all,
            // Debug-only: one line from the webview on the dev terminal, so a
            // `.vue` handler is visible from the shell that runs `tauri dev`.
            commands::debug_log,
            // `webview-control.md` T1: the local MCP endpoint the agent connects
            // to so it can drive those views.
            commands::mcp_endpoint
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Every child webview is closed when the app exits, so nothing of
            // the embedded browser outlives the window that hosts it. This is
            // the `close_all` of the registry: a `Drop` cannot do it, because a
            // dropping registry has no app handle to close a webview with.
            if matches!(event, tauri::RunEvent::Exit) {
                let state = app.state::<AppState>();
                // S4 of `webview-control.md` (T3): the registration names this
                // run's ephemeral port, so it is taken out as the process that
                // owns that port goes away. One short `pi mcp remove`, run here
                // synchronously because a thread would not outlive the exit; a
                // failure leaves an entry that can never connect again, which is
                // the only trace it leaves.
                let _ = commands::unregister_mcp_server(&state);
                state.embeds.close_all(app);
            }
        });
}
