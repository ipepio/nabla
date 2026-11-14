mod commands;
mod settings;
mod store;

use tauri::Manager;

use commands::AppState;
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
            app.manage(AppState { store, config_dir });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_workspaces,
            commands::add_workspace,
            commands::remove_workspace,
            commands::list_sessions,
            commands::get_settings,
            commands::save_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
