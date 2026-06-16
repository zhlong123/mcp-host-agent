use chrono::Local;
use crate::paths::format_audit_path;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

tokio::task_local! {
    pub static CLIENT_IP: String;
}

pub struct AuditLog {
    path: Option<PathBuf>,
    lock: Mutex<()>,
}

impl AuditLog {
    pub fn new(path: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            path,
            lock: Mutex::new(()),
        })
    }

    pub async fn record(&self, tool: &str, path: &Path) {
        let Some(log_path) = &self.path else {
            return;
        };
        let ip = CLIENT_IP
            .try_with(|s| s.clone())
            .unwrap_or_else(|_| "unknown".to_string());
        let path_str = if path.as_os_str().is_empty() || path == Path::new("-") {
            "-".to_string()
        } else {
            format_audit_path(path)
        };
        let line = format!(
            "{}  {}  path={}  ip={}\n",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            tool,
            sanitize_field(&path_str),
            sanitize_field(&ip)
        );
        let _guard = self.lock.lock().await;
        if let Err(e) = append_line(log_path, &line).await {
            tracing::warn!("audit log write failed: {e}");
        }
    }
}

fn sanitize_field(s: &str) -> String {
    s.replace('\n', " ").replace('\r', " ")
}

async fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(line.as_bytes()).await?;
    file.flush().await
}
