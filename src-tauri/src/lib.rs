mod cli;
mod commands;
mod process;
mod providers;
mod rpc;
mod settings;
mod store;

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
            commands::pi_auth_check
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
