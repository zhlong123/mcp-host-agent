use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    #[serde(default = "default_max_read_bytes")]
    pub max_read_bytes: usize,
    #[serde(default = "default_max_write_bytes")]
    pub max_write_bytes: usize,
    #[serde(default = "default_max_list_entries")]
    pub max_list_entries: usize,
    #[serde(default = "default_max_list_depth")]
    pub max_list_depth: usize,
    #[serde(default = "default_max_git_diff_bytes")]
    pub max_git_diff_bytes: usize,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_read_bytes: default_max_read_bytes(),
            max_write_bytes: default_max_write_bytes(),
            max_list_entries: default_max_list_entries(),
            max_list_depth: default_max_list_depth(),
            max_git_diff_bytes: default_max_git_diff_bytes(),
        }
    }
}

fn default_max_read_bytes() -> usize {
    10 * 1024 * 1024
}
fn default_max_write_bytes() -> usize {
    10 * 1024 * 1024
}
fn default_max_list_entries() -> usize {
    10_000
}
fn default_max_list_depth() -> usize {
    10
}
fn default_max_git_diff_bytes() -> usize {
    1024 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootEntry {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub audit_log: Option<PathBuf>,
    /// URL shown to user for Perspective remote agent (e.g. frp tunnel). Local MCP URL is always computed from bind/port.
    #[serde(default)]
    pub public_mcp_url: Option<String>,
    #[serde(default)]
    pub limits: LimitsConfig,
    #[serde(default)]
    pub roots: Vec<RootEntry>,
}

fn default_port() -> u16 {
    9876
}
fn default_bind() -> String {
    "0.0.0.0".to_string()
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            bind: default_bind(),
            token: None,
            audit_log: None,
            public_mcp_url: None,
            limits: LimitsConfig::default(),
            roots: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub port: u16,
    pub bind: String,
    pub token: Option<String>,
    pub audit_log: Option<PathBuf>,
    pub public_mcp_url: Option<String>,
    pub limits: LimitsConfig,
    pub roots: Vec<RootEntry>,
    pub config_path: Option<PathBuf>,
}

pub fn load_config(cli: &CliArgs) -> Result<RuntimeConfig> {
    let mut cfg =     if let Some(path) = &cli.config {
        if path.exists() {
            load_file(path)?
        } else {
            tracing::warn!(
                "config file {} not found — using defaults (save from desktop app to create)",
                path.display()
            );
            FileConfig::default()
        }
    } else if Path::new("agent.toml").exists() {
        load_file("agent.toml")?
    } else if let Ok(exe) = std::env::current_exe() {
        let beside = exe.parent().map(|d| d.join("agent.toml"));
        if let Some(p) = beside.filter(|p| p.exists()) {
            load_file(&p)?
        } else {
            FileConfig::default()
        }
    } else {
        FileConfig::default()
    };

    let config_path = cli
        .config
        .clone()
        .or_else(|| {
            if Path::new("agent.toml").exists() {
                Some(PathBuf::from("agent.toml"))
            } else {
                None
            }
        });

    if let Ok(port) = std::env::var("AGENT_PORT") {
        cfg.port = port.parse().context("invalid AGENT_PORT")?;
    }
    if let Ok(token) = std::env::var("AGENT_TOKEN") {
        if token.is_empty() {
            cfg.token = None;
        } else {
            cfg.token = Some(token);
        }
    }
    if let Ok(root) = std::env::var("AGENT_ROOT") {
        if !root.is_empty() {
            cfg.roots = vec![RootEntry {
                name: "default".to_string(),
                path: PathBuf::from(root),
            }];
        }
    }

    if let Some(port) = cli.port {
        cfg.port = port;
    }
    if let Some(bind) = &cli.bind {
        cfg.bind = bind.clone();
    }
    if let Some(token) = &cli.token {
        if token.is_empty() {
            cfg.token = None;
        } else {
            cfg.token = Some(token.clone());
        }
    }
    for root in &cli.roots {
        cfg.roots.push(RootEntry {
            name: root.name.clone(),
            path: root.path.clone(),
        });
    }

    Ok(RuntimeConfig {
        port: cfg.port,
        bind: cfg.bind,
        token: cfg.token,
        audit_log: cfg.audit_log,
        public_mcp_url: cfg.public_mcp_url,
        limits: cfg.limits,
        roots: cfg.roots,
        config_path,
    })
}

fn load_file(path: impl AsRef<Path>) -> Result<FileConfig> {
    let text = std::fs::read_to_string(path.as_ref())
        .with_context(|| format!("read config {}", path.as_ref().display()))?;
    toml::from_str(&text).context("parse agent.toml")
}

pub fn default_config_path() -> PathBuf {
    if Path::new("agent.toml").exists() {
        PathBuf::from("agent.toml")
    } else if let Ok(exe) = std::env::current_exe() {
        exe.parent()
            .map(|d| d.join("agent.toml"))
            .unwrap_or_else(|| PathBuf::from("agent.toml"))
    } else {
        PathBuf::from("agent.toml")
    }
}

pub fn save_config(path: &Path, cfg: &RuntimeConfig) -> Result<()> {
    let file = FileConfig {
        port: cfg.port,
        bind: cfg.bind.clone(),
        token: cfg.token.clone(),
        audit_log: cfg.audit_log.clone(),
        public_mcp_url: cfg.public_mcp_url.clone(),
        limits: cfg.limits.clone(),
        roots: cfg.roots.clone(),
    };
    let text = toml::to_string_pretty(&file).context("serialize agent.toml")?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
    }
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[derive(Debug, Clone, clap::Parser)]
#[command(name = "perspective-agent", about = "Perspective local MCP agent")]
pub struct CliArgs {
    /// Run MCP HTTP server (used by Tauri sidecar; default when no flag is set)
    #[arg(long)]
    pub serve: bool,

    /// Path to agent.toml
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    #[arg(long)]
    pub port: Option<u16>,

    #[arg(long)]
    pub bind: Option<String>,

    /// Bearer token required on /mcp (empty disables)
    #[arg(long)]
    pub token: Option<String>,

    /// Allowed project root (repeatable): --root name=path
    #[arg(long = "root", value_name = "NAME=PATH", value_parser = parse_root_arg)]
    pub roots: Vec<RootEntry>,
}

fn parse_root_arg(s: &str) -> Result<RootEntry, String> {
    let (name, path) = s
        .split_once('=')
        .ok_or_else(|| "expected NAME=PATH".to_string())?;
    if name.is_empty() {
        return Err("root name must not be empty".to_string());
    }
    Ok(RootEntry {
        name: name.to_string(),
        path: PathBuf::from(path),
    })
}

pub fn load_config_default() -> Result<RuntimeConfig> {
    load_config(&CliArgs {
        serve: false,
        config: None,
        port: None,
        bind: None,
        token: None,
        roots: vec![],
    })
}

pub fn validate_config(cfg: &RuntimeConfig) -> Result<()> {
    if cfg.port == 0 {
        bail!("port must be between 1 and 65535");
    }
    if cfg.bind.trim().is_empty() {
        bail!("bind must not be empty");
    }
    if let Some(url) = &cfg.public_mcp_url {
        if url.trim().is_empty() {
            bail!("public_mcp_url must not be empty when set");
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            bail!("public_mcp_url must start with http:// or https://");
        }
    }
    let l = &cfg.limits;
    if l.max_read_bytes == 0 {
        bail!("limits.max_read_bytes must be > 0");
    }
    if l.max_write_bytes == 0 {
        bail!("limits.max_write_bytes must be > 0");
    }
    if l.max_list_entries == 0 {
        bail!("limits.max_list_entries must be > 0");
    }
    if l.max_git_diff_bytes == 0 {
        bail!("limits.max_git_diff_bytes must be > 0");
    }
    if cfg.roots.is_empty() {
        tracing::warn!(
            "no AGENT_ROOT / [[roots]] configured — all paths allowed (unsafe for tunnel/public exposure)"
        );
    } else {
        for root in &cfg.roots {
            if !root.path.exists() {
                bail!(
                    "root '{}' path does not exist: {}",
                    root.name,
                    root.path.display()
                );
            }
        }
    }
    Ok(())
}
