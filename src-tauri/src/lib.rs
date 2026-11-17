mod browser;
mod cli;
mod commands;
// The embedded browser: one child webview per view, added to the main window.
mod embed;
mod mcp;
mod process;
mod providers;
mod rpc;
mod settings;
mod shell;
mod store;
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
                let envelope = AgentEventEnvelope {
                    session_id: session_id.to_string(),
                    event: event.clone(),
                };
                let _ = handle.emit("agent-event", envelope);
            }));
            // S1/S3 of `pi-cli-bridge.md`: the interactive CLI sessions emit
            // their `{ cliSessionId, stream, line?, exitCode? }` payload on
            // `pi-cli`, the same one-way bridge the runtime uses.
            let cli_handle = app.handle().clone();
            let cli_sink: cli::CliSink = Arc::new(move |event: &cli::CliEvent| {
                let _ = cli_handle.emit("pi-cli", event.clone());
            });
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
                // S4/S6 of `tool-drawer-and-mcp.md` (F3): the Chromium instances
                // Nabla starts itself. The registry is the handle the webview
                // uses, and it kills every child it still owns on drop, so no
                // browser outlives the app.
                browsers: browser::BrowserSessions::new(),
                // The embedded browser's registry: ids and the last url each
                // child webview reported. Tauri owns the webviews themselves —
                // here the app only keeps the metadata it can test without a
                // window.
                embeds: embed::EmbedSessions::new(),
            });
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
            // S2 of `settings-dialog.md`: the dialog shows the preferences path
            // and opens that file with the system editor.
            commands::config_file_path,
            commands::open_config_file,
            commands::start_session,
            // S1/S3/S5: existing sessions are opened lazily, reused when live,
            // and read back from the pi-owned file.
            commands::open_session,
            commands::session_transcript,
            commands::live_session_ids,
            commands::send_prompt,
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
            // F3 of `tool-drawer-and-mcp.md`: the browser the agent drives. One
            // dedicated Chromium launched by Nabla, addressed by session id;
            // no Chromium on the machine means detection answers empty and the
            // tab stays disabled with its reason (S6).
            commands::browser_detect,
            commands::browser_launch,
            commands::browser_targets,
            commands::browser_open_url,
            commands::browser_close,
            // The embedded browser: a child webview of the main window, moved
            // and sized by the UI, with its own lifetime per view. A real
            // webview, not an iframe (X-Frame-Options forbids that).
            commands::embed_create,
            commands::embed_set_bounds,
            commands::embed_navigate,
            commands::embed_eval,
            commands::embed_close,
            commands::embed_url,
            commands::embed_close_all
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Every child webview is closed when the app exits, so nothing of
            // the embedded browser outlives the window that hosts it. This is
            // the `close_all` of the registry: a `Drop` cannot do it, because a
            // dropping registry has no app handle to close a webview with.
            if matches!(event, tauri::RunEvent::Exit) {
                let embeds = app.state::<AppState>().embeds.clone();
                embeds.close_all(app);
            }
        });
}
