mod commands;

use perspective_agent::config::{default_config_path, load_config_default, CliArgs};
use perspective_agent::manager::Manager as AgentManager;
use perspective_agent::serve::run_with_shutdown;
use parking_lot::Mutex;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::watch;

pub struct AppState {
    pub manager: AgentManager,
    server_stop: Mutex<Option<watch::Sender<()>>>,
}

pub fn spawn_mcp_server(state: &Arc<AppState>) -> Result<(), String> {
    if state.manager.status().online {
        return Ok(());
    }
    if state.server_stop.lock().is_some() {
        return Ok(());
    }

    let config_path = state.manager.config_path.clone();
    if !config_path.exists() {
        state
            .manager
            .save()
            .map_err(|e| format!("create config {}: {e}", config_path.display()))?;
    }

    let cli = CliArgs {
        serve: true,
        config: Some(config_path),
        port: None,
        bind: None,
        token: None,
        roots: vec![],
    };

    let (tx, rx) = watch::channel(());
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_with_shutdown(cli, rx).await {
            eprintln!("MCP server stopped: {e}");
        }
    });
    *state.server_stop.lock() = Some(tx);
    Ok(())
}

pub fn stop_mcp_server(state: &Arc<AppState>) -> Result<String, String> {
    if let Some(tx) = state.server_stop.lock().take() {
        let _ = tx.send(());
        Ok("stopped MCP server".into())
    } else {
        Ok("no managed server (external process may still run)".into())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let config = load_config_default().unwrap_or_else(|e| {
                eprintln!("config load failed: {e}, using defaults");
                perspective_agent::config::load_config(&CliArgs {
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
            let manager = AgentManager::new(config, config_path.clone());
            if !config_path.exists() {
                let _ = manager.save();
            }
            let state = Arc::new(AppState {
                manager,
                server_stop: Mutex::new(None),
            });
            app.manage(Arc::clone(&state));
            if let Err(e) = spawn_mcp_server(&state) {
                eprintln!("auto-start MCP failed: {e}");
            }
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
