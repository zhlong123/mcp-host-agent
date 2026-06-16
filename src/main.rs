//! CLI binary: MCP HTTP server (sidecar for Tauri desktop app)

use anyhow::Context;
use clap::Parser;
use mcp_host_agent::config::CliArgs;
use mcp_host_agent::install_panic_log;
use mcp_host_agent::serve::run as run_server;

fn main() -> anyhow::Result<()> {
    install_panic_log();
    let cli = CliArgs::parse();
    let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
    rt.block_on(run_server(cli))
}
