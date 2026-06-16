//! MCP Host Agent core library (MCP server + config + manager)

pub mod activity;
pub mod audit;
pub mod config;
pub mod manager;
pub mod paths;
pub mod search;
pub mod serve;

pub use activity::{ActivityEvent, ActivityLog, ActivityQuery, DiffSummary, OpKind};

pub fn install_panic_log() {
    use std::path::PathBuf;
    let exe = std::env::current_exe().ok();
    let log_path = exe
        .as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.join("mcp-host-agent-panic.log"))
        .unwrap_or_else(|| PathBuf::from("mcp-host-agent-panic.log"));
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!(
            "[{}] PANIC: {}\nbacktrace:\n{}\n",
            chrono::Utc::now().to_rfc3339(),
            info,
            std::backtrace::Backtrace::capture()
        );
        let _ = std::fs::write(&log_path, &msg);
        eprintln!("{msg}");
    }));
}
