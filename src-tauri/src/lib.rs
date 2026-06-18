mod commands;
mod tunnel;

use mcp_host_agent::activity::{ActivityLog, default_activity_log_path};
use mcp_host_agent::config::{default_config_path, load_config_default, CliArgs};
use mcp_host_agent::manager::Manager as AgentManager;
use mcp_host_agent::serve::run_with_shutdown;
use parking_lot::Mutex;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::watch;

pub struct AppState {
    pub manager: AgentManager,
    pub activity: Arc<ActivityLog>,
    pub tunnel: Arc<tunnel::QuickTunnel>,
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

    let activity = Arc::clone(&state.activity);
    let (tx, rx) = watch::channel(());
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_with_shutdown(cli, rx, Some(activity)).await {
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

pub async fn restart_mcp_server(state: &Arc<AppState>) -> Result<String, String> {
    stop_mcp_server(state)?;
    let port = state.manager.get_config().port;
    for _ in 0..40 {
        if mcp_host_agent::serve::probe_health(port).is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if mcp_host_agent::serve::probe_health(port).is_some() {
        return Err("port still in use — stop external MCP process first".into());
    }
    spawn_mcp_server(state)?;
    Ok("restarting MCP server…".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let config = load_config_default().unwrap_or_else(|e| {
                eprintln!("config load failed: {e}, using defaults");
                mcp_host_agent::config::load_config(&CliArgs {
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
            let activity_path = config.activity_log.clone().or_else(default_activity_log_path);
            let activity = Arc::new(ActivityLog::open(activity_path));
            let manager = AgentManager::new(config, config_path.clone());
            if !config_path.exists() {
                let _ = manager.save();
            }
            let state = Arc::new(AppState {
                manager,
                activity,
                tunnel: Arc::new(tunnel::QuickTunnel::new()),
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
            commands::restart_server,
            commands::get_audit_logs,
            commands::get_activity_events,
            commands::pick_folder,
            commands::get_tunnel_status,
            commands::start_quick_tunnel,
            commands::stop_quick_tunnel,
        ])
        .build(tauri::generate_context!())
        .expect("tauri build failed")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<Arc<AppState>>() {
                    let _ = state.tunnel.stop();
                }
            }
        })
}
