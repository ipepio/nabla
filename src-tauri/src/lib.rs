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
        .setup(|app| {
            // Directories are resolved once at startup: Tauri maps them to the
            // platform-specific locations, so no path is hardcoded here.
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            // SQLite creates the file but not its parent directory.
            std::fs::create_dir_all(&data_dir)?;
            let store = Store::open(&data_dir.join("nabla.db"))?;
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
            commands::list_sessions,
            commands::get_settings,
            commands::save_settings,
            commands::start_session,
            commands::send_prompt,
            commands::stop_session,
            commands::session_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
