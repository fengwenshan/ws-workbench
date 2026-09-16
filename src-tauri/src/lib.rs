mod models;
mod config;
mod commands;
mod packer;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::get_config_path,
            commands::pack_project,
            commands::pack_to_zip,
            commands::validate_project,
            commands::detect_project_type,
            commands::open_dir,
            commands::open_parent_dir,
            commands::open_in_vscode,
            commands::open_in_idea,
            commands::open_in_trae,
            commands::open_in_terminal,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
