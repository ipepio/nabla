mod commands;
mod process;
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
            app.manage(AppState {
                store,
                config_dir,
                data_dir,
                runtime,
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
            commands::session_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
