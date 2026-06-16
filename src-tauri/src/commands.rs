use crate::{stop_mcp_server, spawn_mcp_server, AppState};
use perspective_agent::config::RuntimeConfig;
use perspective_agent::manager::StatusResponse;
use std::sync::Arc;
use tauri::{State};
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub fn get_status(state: State<Arc<AppState>>) -> StatusResponse {
    let mut s = state.manager.status();
    if state.server_stop.lock().is_some() {
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
pub fn start_server(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    spawn_mcp_server(&state)?;
    Ok("starting MCP server…".into())
}

#[tauri::command]
pub fn stop_server(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    stop_mcp_server(&state)
}

#[tauri::command]
pub fn get_audit_logs(state: State<Arc<AppState>>) -> Vec<String> {
    state.manager.audit_tail(200)
}

#[tauri::command]
pub async fn pick_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let picked = app.dialog().file().blocking_pick_folder();
    Ok(picked.map(|p| p.to_string()))
}
