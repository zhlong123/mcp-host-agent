mod commands;

use perspective_agent::config::{default_config_path, load_config_default};
use perspective_agent::manager::Manager;
use parking_lot::Mutex;
use std::sync::Arc;
use tauri::Manager as _;

pub struct AppState {
    pub manager: Manager,
    pub sidecar: Mutex<Option<tauri_plugin_shell::process::CommandChild>>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let config = load_config_default().unwrap_or_else(|e| {
                eprintln!("config load failed: {e}, using defaults");
                perspective_agent::config::load_config(&perspective_agent::config::CliArgs {
                    serve: false,
                    config: None,
                    port: None,
                    bind: None,
                    token: None,
                    roots: vec![],
                })
                .expect("default config")
            });
            let config_path = config
                .config_path
                .clone()
                .unwrap_or_else(default_config_path);
            let manager = Manager::new(config, config_path);
            app.manage(Arc::new(AppState {
                manager,
                sidecar: Mutex::new(None),
            }));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::get_config,
            commands::save_config,
            commands::start_server,
            commands::stop_server,
            commands::get_audit_logs,
            commands::pick_folder,
        ])
        .run(tauri::generate_context!())
        .expect("tauri run failed");
}
