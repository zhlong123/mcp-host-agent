//! Shared config / server process management for the Tauri desktop app

use crate::config::{RuntimeConfig, save_config, validate_config};
use crate::serve::probe_health;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Manager {
    pub config_path: PathBuf,
    inner: Arc<ManagerInner>,
}

struct ManagerInner {
    config: Mutex<RuntimeConfig>,
    server_child: Mutex<Option<Child>>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct StatusResponse {
    pub online: bool,
    pub detail: String,
    pub version: Option<String>,
    pub git_available: Option<bool>,
    /// MCP URL for clients (public_mcp_url if set, else local)
    pub mcp_url: String,
    pub local_mcp_url: String,
    pub public_mcp_url: Option<String>,
    pub health_url: String,
    pub config_path: String,
    pub managed_server: bool,
}

impl Manager {
    pub fn new(config: RuntimeConfig, config_path: PathBuf) -> Self {
        Self {
            config_path,
            inner: Arc::new(ManagerInner {
                config: Mutex::new(config),
                server_child: Mutex::new(None),
            }),
        }
    }

    pub fn get_config(&self) -> RuntimeConfig {
        self.inner.config.lock().unwrap().clone()
    }

    pub fn set_config(&self, config: RuntimeConfig) -> Result<()> {
        validate_config(&config)?;
        *self.inner.config.lock().unwrap() = config;
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        let cfg = self.get_config();
        save_config(&self.config_path, &cfg)
    }

    pub fn status(&self) -> StatusResponse {
        let cfg = self.get_config();
        let host = if cfg.bind == "0.0.0.0" {
            "127.0.0.1".to_string()
        } else {
            cfg.bind.clone()
        };
        let local_mcp_url = format!("http://{host}:{}/mcp", cfg.port);
        let mcp_url = cfg
            .public_mcp_url
            .clone()
            .unwrap_or_else(|| local_mcp_url.clone());
        let health_url = format!("http://127.0.0.1:{}/health", cfg.port);
        let managed = self.inner.server_child.lock().unwrap().is_some();

        if let Some(h) = probe_health(cfg.port) {
            StatusResponse {
                online: true,
                detail: format!(
                    "v{} · git={} · {} roots · read={}MiB write={}MiB",
                    h.version,
                    h.git_available,
                    h.roots.len(),
                    cfg.limits.max_read_bytes / (1024 * 1024),
                    cfg.limits.max_write_bytes / (1024 * 1024),
                ),
                version: Some(h.version),
                git_available: Some(h.git_available),
                mcp_url,
                local_mcp_url,
                public_mcp_url: cfg.public_mcp_url.clone(),
                health_url,
                config_path: self.config_path.display().to_string(),
                managed_server: managed,
            }
        } else {
            StatusResponse {
                online: false,
                detail: "offline — start MCP service".to_string(),
                version: None,
                git_available: None,
                mcp_url,
                local_mcp_url,
                public_mcp_url: cfg.public_mcp_url.clone(),
                health_url,
                config_path: self.config_path.display().to_string(),
                managed_server: managed,
            }
        }
    }

    pub fn start_server(&self) -> Result<String> {
        if self.status().online {
            return Ok("server already running".into());
        }
        let cfg = self.get_config();
        let exe = std::env::current_exe().context("current_exe")?;
        let mut cmd = Command::new(exe);
        cmd.arg("--serve").arg("--config").arg(&self.config_path);
        if cfg.port != 9876 {
            cmd.arg("--port").arg(cfg.port.to_string());
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let child = cmd.spawn().context("spawn MCP server")?;
        *self.inner.server_child.lock().unwrap() = Some(child);
        Ok("starting MCP server…".into())
    }

    pub fn stop_server(&self) -> String {
        if let Some(mut child) = self.inner.server_child.lock().unwrap().take() {
            let _ = child.kill();
            "stopped managed server".into()
        } else {
            "no managed server (external process may still run)".into()
        }
    }

    pub fn audit_tail(&self, lines: usize) -> Vec<String> {
        let cfg = self.get_config();
        let path = cfg
            .audit_log
            .clone()
            .or_else(default_audit_log_path);
        let Some(path) = path else {
            return vec!["(no audit log path)".into()];
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return vec!["(audit log empty or missing)".into()];
        };
        text.lines()
            .rev()
            .take(lines)
            .map(str::to_string)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }
}

pub fn default_audit_log_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("mcp-host-agent-audit.log")))
}
