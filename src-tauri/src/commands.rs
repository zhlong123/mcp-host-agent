use crate::AppState;
use perspective_agent::config::RuntimeConfig;
use perspective_agent::manager::StatusResponse;
use std::sync::Arc;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_shell::ShellExt;

#[tauri::command]
pub fn get_status(state: State<Arc<AppState>>) -> StatusResponse {
    let mut s = state.manager.status();
    if state.sidecar.lock().is_some() {
        s.managed_server = true;
    }
    s
}

#[tauri::command]
pub fn get_config(state: State<Arc<AppState>>) -> RuntimeConfig {
    state.manager.get_config()
}

#[tauri::command]
pub fn save_config(
    state: State<Arc<AppState>>,
    config: RuntimeConfig,
) -> Result<String, String> {
    state
        .manager
        .set_config(config)
        .map_err(|e| e.to_string())?;
    state.manager.save().map_err(|e| e.to_string())?;
    Ok("saved".into())
}

#[tauri::command]
pub async fn start_server(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    if state.manager.status().online {
        return Ok("server already running".into());
    }
    if state.sidecar.lock().is_some() {
        return Ok("server already starting".into());
    }

    let config_path = state.manager.config_path.clone();
    let config_arg = config_path.to_string_lossy().to_string();

    let (_rx, sidecar) = app
        .shell()
        .sidecar("perspective-agent")
        .map_err(|e| e.to_string())?
        .args(["--serve", "--config", &config_arg])
        .spawn()
        .map_err(|e| e.to_string())?;

    *state.sidecar.lock() = Some(sidecar);
    Ok("starting MCP server…".into())
}

#[tauri::command]
pub fn stop_server(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    if let Some(child) = state.sidecar.lock().take() {
        child.kill().map_err(|e| e.to_string())?;
        Ok("stopped MCP server".into())
    } else {
        Ok("no managed server (external process may still run)".into())
    }
}

#[tauri::command]
pub fn get_audit_logs(state: State<Arc<AppState>>) -> Vec<String> {
    state.manager.audit_tail(200)
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    let picked = app
        .dialog()
        .file()
        .blocking_pick_folder();
    Ok(picked.map(|p| p.to_string()))
}
