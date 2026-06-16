//! MCP HTTP server (default mode)

use crate::audit::{AuditLog, CLIENT_IP};
use crate::config::{CliArgs, RuntimeConfig, load_config, validate_config};
use crate::paths::{mcp_err, resolve_allowed_path};
use anyhow::Result;
use axum::{
    Router,
    extract::{ConnectInfo, State},
    http::{Request, StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{IntoResponse, Json},
    routing::get,
};
use base64::Engine;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::Parameters},
    model::{ErrorData, ServerCapabilities, ServerInfo},
    schemars,
    transport::streamable_http_server::{
        StreamableHttpService, session::local::LocalSessionManager,
    },
    tool, tool_handler, tool_router,
};
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{future::Future, net::SocketAddr, path::PathBuf, process::Stdio, sync::Arc};
use tokio::process::Command;
use tracing::{info, warn};

#[derive(Clone)]
struct AppState {
    config: Arc<RuntimeConfig>,
    canon_roots: Vec<(String, PathBuf)>,
    git_available: bool,
    audit: Arc<AuditLog>,
}

#[derive(Clone)]
struct Agent {
    tool_router: ToolRouter<Self>,
    state: Arc<AppState>,
}

impl Agent {
    fn new(state: Arc<AppState>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            state,
        }
    }

    async fn allowed_path(&self, tool: &str, user_path: &str) -> Result<PathBuf, ErrorData> {
        let path = resolve_allowed_path(user_path, &self.state.canon_roots)?;
        self.state.audit.record(tool, user_path).await;
        Ok(path)
    }

    fn ensure_git(&self) -> Result<(), ErrorData> {
        if self.state.git_available {
            Ok(())
        } else {
            Err(mcp_err(
                -32013,
                "git not available on this machine (install Git and ensure it is in PATH)",
            ))
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Agent {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::default(),
            server_info: rmcp::model::Implementation {
                name: "perspective-agent".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            instructions: Some(
                "Perspective agent: file/git tools scoped to configured roots. \
                 ~ expands to $HOME / %USERPROFILE%."
                    .to_string(),
            ),
            ..Default::default()
        }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct PingOutput {
    pong: String,
    version: String,
    git_available: bool,
    roots: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ReadFileArgs {
    path: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ReadFileOutput {
    content_b64: String,
    size_bytes: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WriteFileArgs {
    path: String,
    content_b64: String,
    if_mtime_unix_ms: Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WriteFileOutput {
    new_mtime_unix_ms: i64,
    bytes_written: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListDirArgs {
    path: String,
    #[serde(default)]
    recursive: bool,
    #[serde(default)]
    max_depth: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct DirEntry {
    name: String,
    kind: String,
    size_bytes: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ListDirOutput {
    entries: Vec<DirEntry>,
    total: usize,
    truncated: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct StatArgs {
    path: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct StatOutput {
    exists: bool,
    kind: Option<String>,
    size_bytes: Option<u64>,
    mtime_unix_ms: Option<i64>,
    is_readonly: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GitStatusArgs {
    path: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct GitStatusOutput {
    is_git: bool,
    branch: Option<String>,
    uncommitted: i64,
    ahead: i64,
    behind: i64,
    last_commit: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GitDiffArgs {
    path: String,
    #[serde(default)]
    staged: bool,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct GitDiffOutput {
    diff: String,
    truncated: bool,
    bytes: usize,
}

#[derive(Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub git_available: bool,
    pub roots: Vec<String>,
    pub mcp: String,
    pub ping: String,
}

#[tool_router(router = tool_router)]
impl Agent {
    #[tool(name = "ping", description = "Connectivity test; also usable for tunnel probes via MCP")]
    async fn ping(&self) -> String {
        self.state.audit.record("ping", "-").await;
        let roots: Vec<String> = self
            .state
            .canon_roots
            .iter()
            .map(|(n, p)| format!("{n}={}", p.display()))
            .collect();
        serde_json::to_string(&PingOutput {
            pong: "pong".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            git_available: self.state.git_available,
            roots,
        })
        .unwrap_or_default()
    }

    #[tool(name = "read_file", description = "Read file (base64)")]
    async fn read_file(&self, Parameters(args): Parameters<ReadFileArgs>) -> Result<String, ErrorData> {
        let path = self.allowed_path("read_file", &args.path).await?;
        let max = self.state.config.limits.max_read_bytes;
        if let Ok(meta) = tokio::fs::metadata(&path).await {
            if meta.len() as usize > max {
                return Err(mcp_err(
                    -32007,
                    format!("file too large: {} bytes (max_read_bytes={max})", meta.len()),
                ));
            }
        }
        let data = tokio::fs::read(&path)
            .await
            .map_err(|e| mcp_err(-32001, format!("read {} failed: {e}", path.display())))?;
        if data.len() > max {
            return Err(mcp_err(-32007, format!("file too large after read (max={max})")));
        }
        Ok(serde_json::to_string(&ReadFileOutput {
            content_b64: base64::engine::general_purpose::STANDARD.encode(&data),
            size_bytes: data.len() as u64,
        })
        .unwrap_or_default())
    }

    #[tool(name = "write_file", description = "Write file (base64); mtime param accepted but not enforced in agent mode")]
    async fn write_file(
        &self,
        Parameters(args): Parameters<WriteFileArgs>,
    ) -> Result<String, ErrorData> {
        let path = self.allowed_path("write_file", &args.path).await?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(&args.content_b64)
            .map_err(|e| mcp_err(-32002, format!("base64 decode failed: {e}")))?;
        let max = self.state.config.limits.max_write_bytes;
        if data.len() > max {
            return Err(mcp_err(
                -32008,
                format!("payload too large: {} bytes (max_write_bytes={max})", data.len()),
            ));
        }
        if args.if_mtime_unix_ms.is_some() {
            tracing::debug!(
                "write_file if_mtime_unix_ms ignored in agent mode (INTEGRATION simplified)"
            );
        }
        tokio::fs::write(&path, &data)
            .await
            .map_err(|e| mcp_err(-32004, format!("write {} failed: {e}", path.display())))?;
        let new_mtime = tokio::fs::metadata(&path)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        Ok(serde_json::to_string(&WriteFileOutput {
            new_mtime_unix_ms: new_mtime,
            bytes_written: data.len(),
        })
        .unwrap_or_default())
    }

    #[tool(name = "list_dir", description = "List directory (optional recursive)")]
    async fn list_dir(&self, Parameters(args): Parameters<ListDirArgs>) -> Result<String, ErrorData> {
        let path = self.allowed_path("list_dir", &args.path).await?;
        let mut entries = Vec::new();
        let cfg_max_depth = self.state.config.limits.max_list_depth;
        let max_entries = self.state.config.limits.max_list_entries;
        let req_depth = args.max_depth.unwrap_or(if args.recursive { 3 } else { 0 });
        let max_depth = if req_depth == 0 {
            req_depth
        } else {
            req_depth.min(cfg_max_depth)
        };
        let mut truncated = false;

        fn walk(
            dir: &std::path::Path,
            entries: &mut Vec<DirEntry>,
            current_depth: usize,
            max_depth: usize,
            max_entries: usize,
            truncated: &mut bool,
        ) -> std::io::Result<()> {
            if *truncated {
                return Ok(());
            }
            for e in std::fs::read_dir(dir)? {
                if entries.len() >= max_entries {
                    *truncated = true;
                    return Ok(());
                }
                let e = e?;
                let meta = match e.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let kind = if meta.is_symlink() {
                    "symlink"
                } else if meta.is_dir() {
                    "dir"
                } else if meta.is_file() {
                    "file"
                } else {
                    "other"
                };
                entries.push(DirEntry {
                    name: e.file_name().to_string_lossy().to_string(),
                    kind: kind.to_string(),
                    size_bytes: meta.len(),
                });
                if meta.is_dir() && (max_depth == 0 || current_depth < max_depth) {
                    let _ = walk(&e.path(), entries, current_depth + 1, max_depth, max_entries, truncated);
                }
            }
            Ok(())
        }

        walk(&path, &mut entries, 1, max_depth, max_entries, &mut truncated)
            .map_err(|e| mcp_err(-32005, format!("list {} failed: {e}", path.display())))?;
        let total = entries.len();
        Ok(serde_json::to_string(&ListDirOutput {
            entries,
            total,
            truncated,
        })
        .unwrap_or_default())
    }

    #[tool(name = "stat", description = "File metadata")]
    async fn stat(&self, Parameters(args): Parameters<StatArgs>) -> Result<String, ErrorData> {
        let path = self.allowed_path("stat", &args.path).await?;
        match tokio::fs::metadata(&path).await {
            Ok(meta) => {
                let kind = if meta.is_dir() {
                    "dir"
                } else if meta.is_file() {
                    "file"
                } else {
                    "other"
                };
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64);
                Ok(serde_json::to_string(&StatOutput {
                    exists: true,
                    kind: Some(kind.to_string()),
                    size_bytes: Some(meta.len()),
                    mtime_unix_ms: mtime,
                    is_readonly: Some(meta.permissions().readonly()),
                })
                .unwrap_or_default())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::to_string(&StatOutput {
                exists: false,
                kind: None,
                size_bytes: None,
                mtime_unix_ms: None,
                is_readonly: None,
            })
            .unwrap_or_default()),
            Err(e) => Err(mcp_err(-32006, format!("stat {} failed: {e}", path.display()))),
        }
    }

    #[tool(name = "git_status", description = "Git status: branch / uncommitted / ahead / behind (upstream) / last commit")]
    async fn git_status(
        &self,
        Parameters(args): Parameters<GitStatusArgs>,
    ) -> Result<String, ErrorData> {
        self.ensure_git()?;
        let path = self.allowed_path("git_status", &args.path).await?;
        let repo = path.to_str().unwrap_or(".");
        let porcelain = Command::new("git")
            .args(["-C", repo, "status", "--porcelain"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;
        let porcelain_out = match porcelain {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            Ok(o) => {
                return Ok(serde_json::to_string(&GitStatusOutput {
                    is_git: false,
                    branch: None,
                    uncommitted: 0,
                    ahead: 0,
                    behind: 0,
                    last_commit: None,
                    error: Some(String::from_utf8_lossy(&o.stderr).to_string()),
                })
                .unwrap_or_default());
            }
            Err(e) => return Err(mcp_err(-32010, format!("git failed to start: {e}"))),
        };
        let uncommitted = porcelain_out.lines().filter(|l| !l.is_empty()).count() as i64;
        let branch = git_stdout(&["-C", repo, "branch", "--show-current"]).await;
        let (ahead, behind) = git_ahead_behind(repo).await;
        let last_commit = git_stdout(&["-C", repo, "rev-parse", "--short", "HEAD"]).await;
        Ok(serde_json::to_string(&GitStatusOutput {
            is_git: true,
            branch,
            uncommitted,
            ahead,
            behind,
            last_commit,
            error: None,
        })
        .unwrap_or_default())
    }

    #[tool(name = "git_diff", description = "Git diff (optional --staged)")]
    async fn git_diff(
        &self,
        Parameters(args): Parameters<GitDiffArgs>,
    ) -> Result<String, ErrorData> {
        self.ensure_git()?;
        let path = self.allowed_path("git_diff", &args.path).await?;
        let repo = path.to_str().unwrap_or(".");
        let max_bytes = args
            .max_bytes
            .unwrap_or(self.state.config.limits.max_git_diff_bytes)
            .min(self.state.config.limits.max_git_diff_bytes);
        let mut cmd = Command::new("git");
        cmd.args(["-C", repo, "diff"]);
        if args.staged {
            cmd.arg("--staged");
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = cmd
            .output()
            .await
            .map_err(|e| mcp_err(-32011, format!("git diff failed to start: {e}")))?;
        if !out.status.success() {
            return Err(mcp_err(
                -32012,
                format!("git diff failed: {}", String::from_utf8_lossy(&out.stderr)),
            ));
        }
        let full = String::from_utf8_lossy(&out.stdout).to_string();
        let (truncated, body) = if full.len() > max_bytes {
            (true, full[..max_bytes].to_string())
        } else {
            (false, full)
        };
        Ok(serde_json::to_string(&GitDiffOutput {
            bytes: body.len(),
            truncated,
            diff: body,
        })
        .unwrap_or_default())
    }
}

async fn git_stdout(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

async fn git_rev_count(repo: &str, range: &str) -> i64 {
    Command::new("git")
        .args(["-C", repo, "rev-list", "--count", range])
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0)
}

async fn git_ahead_behind(repo: &str) -> (i64, i64) {
    let upstream = git_stdout(&["-C", repo, "rev-parse", "--abbrev-ref", "@{upstream}"]).await;
    let Some(upstream) = upstream else {
        return (0, 0);
    };
    let ahead = git_rev_count(repo, &format!("{upstream}..HEAD")).await;
    let behind = git_rev_count(repo, &format!("HEAD..{upstream}")).await;
    (ahead, behind)
}

fn client_ip<B>(req: &Request<B>, connect: Option<ConnectInfo<SocketAddr>>) -> String {
    req.headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .or_else(|| {
            req.headers()
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
        })
        .map(str::to_string)
        .or_else(|| connect.map(|ConnectInfo(addr)| addr.ip().to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

async fn mcp_middleware(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<axum::body::Body>,
    next: Next,
) -> axum::response::Response {
    if let Some(expected) = &state.config.token {
        let authorized = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(|h| h.strip_prefix("Bearer ").unwrap_or(h).trim() == expected.as_str())
            .unwrap_or(false);
        if !authorized {
            return (StatusCode::UNAUTHORIZED, "missing or invalid Bearer token").into_response();
        }
    }
    let ip = client_ip(&req, Some(ConnectInfo(addr)));
    CLIENT_IP.scope(ip, async move { next.run(req).await }).await
}

async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_available: state.git_available,
        roots: state
            .canon_roots
            .iter()
            .map(|(n, p)| format!("{n}={}", p.display()))
            .collect(),
        mcp: "/mcp".to_string(),
        ping: "MCP tool ping or GET /health for tunnel probes".to_string(),
    })
}

async fn index(State(state): State<Arc<AppState>>) -> axum::response::Html<String> {
    let port = state.config.port;
    let roots = state
        .canon_roots
        .iter()
        .map(|(n, p)| format!("<li><code>{n}</code> → {}</li>", html_escape(&p.display().to_string())))
        .collect::<String>();
    let roots_html = if roots.is_empty() {
        "<li>（未配置沙箱，允许全部路径）</li>".to_string()
    } else {
        roots
    };
    axum::response::Html(format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN"><head><meta charset="utf-8"><title>Perspective Agent</title>
<style>
body{{font-family:Segoe UI,sans-serif;background:#0f1419;color:#e8eef7;margin:0;padding:32px;line-height:1.6}}
.card{{max-width:720px;background:#1a2332;border:1px solid #2d3f58;border-radius:12px;padding:24px}}
.ok{{color:#34d399}} code{{background:#243044;padding:2px 6px;border-radius:4px}}
a{{color:#3d9cf5}} ul{{padding-left:20px}}
</style></head><body>
<div class="card">
<h1>Perspective Agent <span class="ok">● ONLINE</span></h1>
<p>这是 <strong>MCP 后端服务</strong>，不是图形管理页面。浏览器里只能查看本说明和探活接口。</p>
<h2>怎么用</h2>
<ul>
<li><strong>图形管理</strong>：运行 <code>perspective-agent-app.exe</code>（桌面窗口）</li>
<li><strong>探活 JSON</strong>：<a href="/health">/health</a></li>
<li><strong>MCP 接口</strong>：<code>/mcp</code>（给 Perspective 用，浏览器直接打开会 406，属正常）</li>
<li><strong>填到 Perspective</strong>：<code>http://127.0.0.1:{port}/mcp</code> 或穿透地址</li>
</ul>
<h2>当前状态</h2>
<ul>
<li>版本：{version}</li>
<li>Git 工具：{git}</li>
<li>沙箱 roots：{roots_html}</li>
</ul>
<p style="color:#8fa3bf;font-size:14px">Windows 请用 <code>127.0.0.1:{port}</code>，不要用 <code>localhost</code>（会走 IPv6）。图形管理请开 <code>perspective-agent-app.exe</code>。</p>
</div></body></html>"#,
        version = env!("CARGO_PKG_VERSION"),
        git = if state.git_available { "可用" } else { "不可用" },
        roots_html = roots_html,
    ))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn default_audit_log_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("perspective-agent-audit.log")))
}

pub fn probe_health(port: u16) -> Option<HealthResponse> {
    let url = format!("http://127.0.0.1:{port}/health");
    let response = minreq::get(url).with_timeout(2).send().ok()?;
    if response.status_code != 200 {
        return None;
    }
    let body = response.as_str().ok()?;
    serde_json::from_str(body).ok()
}

pub async fn run(cli: CliArgs) -> Result<()> {
    let (_tx, rx) = tokio::sync::watch::channel(());
    run_with_shutdown(cli, rx).await
}

pub async fn run_with_shutdown(
    cli: CliArgs,
    mut shutdown: tokio::sync::watch::Receiver<()>,
) -> Result<()> {
    let runtime = load_config(&cli)?;
    validate_config(&runtime)?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,perspective_agent=debug")),
        )
        .try_init()
        .ok();

    if let Some(path) = &runtime.config_path {
        info!("loaded config: {}", path.display());
    }

    let git_available = which::which("git").is_ok();
    if git_available {
        info!("git detected in PATH — git_status / git_diff enabled");
    } else {
        warn!("git not found in PATH — git_status / git_diff disabled");
    }

    let canon_roots = crate::paths::canonical_roots(&runtime.roots);
    if !runtime.roots.is_empty() {
        info!("path sandbox: {} root(s) configured", canon_roots.len());
        for (name, path) in &canon_roots {
            info!("  allowed root {name}: {}", path.display());
        }
    }

    if runtime.token.is_some() {
        info!("Bearer token auth enabled on /mcp");
    } else {
        warn!("no token configured — /mcp is open to anyone who can reach this port");
    }

    let audit_path = runtime.audit_log.clone().or_else(default_audit_log_path);
    if let Some(audit) = &audit_path {
        info!("audit log: {}", audit.display());
    }

    let runtime_cfg = Arc::new(runtime);
    let state = Arc::new(AppState {
        config: Arc::clone(&runtime_cfg),
        canon_roots,
        git_available,
        audit: AuditLog::new(audit_path),
    });

    let agent = Agent::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(agent.clone()),
        LocalSessionManager::default().into(),
        Default::default(),
    );

    let mcp_routes = Router::new()
        .nest_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            mcp_middleware,
        ));

    let app = Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .merge(mcp_routes)
        .with_state(state);

    let make_svc = || app.clone().into_make_service_with_connect_info::<SocketAddr>();
    let local = format!("127.0.0.1:{}", runtime_cfg.port);
    info!("health probe: http://{local}/health");
    info!("MCP endpoint: http://{local}/mcp");

    if runtime_cfg.bind == "0.0.0.0" {
        let v4 = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", runtime_cfg.port)).await?;
        info!("listening on 0.0.0.0:{} (IPv4)", runtime_cfg.port);
        match tokio::net::TcpListener::bind(format!("[::]:{}", runtime_cfg.port)).await {
            Ok(v6) => {
                info!("listening on [::]:{} (IPv6 / localhost)", runtime_cfg.port);
                let svc = make_svc();
                let mut sd6 = shutdown.clone();
                tokio::spawn(async move {
                    if let Err(e) = axum::serve(v6, svc)
                        .with_graceful_shutdown(async move {
                            let _ = sd6.changed().await;
                        })
                        .await
                    {
                        tracing::error!("IPv6 listener stopped: {e}");
                    }
                });
            }
            Err(e) => warn!("IPv6 bind skipped: {e}"),
        }
        axum::serve(v4, make_svc())
            .with_graceful_shutdown(async move {
                let _ = shutdown.changed().await;
            })
            .await?;
    } else {
        let listener = bind_listener(&runtime_cfg.bind, runtime_cfg.port).await?;
        axum::serve(listener, make_svc())
            .with_graceful_shutdown(async move {
                let _ = shutdown.changed().await;
            })
            .await?;
    }
    Ok(())
}

async fn bind_listener(bind: &str, port: u16) -> Result<tokio::net::TcpListener> {
    let addr: SocketAddr = format!("{bind}:{port}").parse()?;
    Ok(tokio::net::TcpListener::bind(addr).await?)
}
