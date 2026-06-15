//! Perspective Agent — 本机 MCP server
//!
//! 暴露文件 + git 工具给 Perspective server(本机版 — agent 跟 server 同机,绑 127.0.0.1)
//!
//! 工具集:
//!   - ping              连通性测试
//!   - read_file         读文件(base64 返)
//!   - write_file        写文件(可选 mtime 冲突检测)
//!   - list_dir          列目录(支持 recursive)
//!   - stat              文件元信息
//!   - git_status        git 探测(uncommitted / ahead / behind / branch / last commit)
//!   - git_diff          git diff(可选 --staged)
//!
//! 设计原则:
//!   - 所有路径都当成本机路径(解析 ~)
//!   - 错误返 rmcp::ErrorData,code = -32000 系列自定义
//!   - 文件 op 全部走 tokio::fs,async 不阻塞
//!   - git op 走 git CLI subprocess,parse stdout
//!
//! 鉴权(Y 路径再加):本机版先不加,bind 127.0.0.1 够了

use anyhow::{Context, Result};
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
use std::{future::Future, net::SocketAddr, path::PathBuf, process::Stdio};
use tokio::process::Command;
use tracing::{info, warn};

// ───── Agent struct ─────

#[derive(Debug, Clone)]
struct Agent {
    tool_router: ToolRouter<Self>,
}

impl Default for Agent {
    fn default() -> Self {
        Self {
            tool_router: Self::tool_router(),
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
                "Perspective 本机 agent,暴露文件 / git 工具给 Perspective server。\
                 所有路径都是 agent 本机路径,~ 会被展开成 $HOME。"
                    .to_string(),
            ),
            ..Default::default()
        }
    }
}

// ───── 工具实现 ─────

/// 展开 ~ → $HOME
fn expand_path(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    } else if p == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(p)
}

/// rmcp 错误:code = -32000 是 MCP 保留给"server-defined error"的
fn mcp_err(code: i32, msg: impl Into<String>) -> ErrorData {
    ErrorData::new(rmcp::model::ErrorCode(code), msg.into(), None)
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct PingOutput {
    pong: String,
    version: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ReadFileArgs {
    /// 文件绝对路径,~ 会被展开
    path: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ReadFileOutput {
    /// base64 编码的内容(避免 JSON 二进制乱码)
    content_b64: String,
    size_bytes: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WriteFileArgs {
    /// 文件绝对路径
    path: String,
    /// base64 编码的内容
    content_b64: String,
    /// 若提供,只在该 mtime 时才写(并发冲突检测)
    if_mtime_unix_ms: Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WriteFileOutput {
    new_mtime_unix_ms: i64,
    bytes_written: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListDirArgs {
    /// 目录绝对路径
    path: String,
    /// 是否递归(默认 false)
    #[serde(default)]
    recursive: bool,
    /// 最大深度(只 recursive 时生效,默认 3,0 = 不限)
    #[serde(default)]
    max_depth: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct DirEntry {
    name: String,
    /// "file" | "dir" | "symlink" | "other"
    kind: String,
    size_bytes: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ListDirOutput {
    entries: Vec<DirEntry>,
    total: usize,
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
    /// 项目根路径(必须是 git repo)
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
    /// 只看已暂存(--staged)
    #[serde(default)]
    staged: bool,
    /// 最大返回字节数,超出截断(默认 1MB)
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct GitDiffOutput {
    diff: String,
    truncated: bool,
    bytes: usize,
}

#[tool_router(router = tool_router)]
impl Agent {
    /// 连通性测试
    #[tool(name = "ping", description = "连通性测试,返回 pong")]
    async fn ping(&self) -> String {
        serde_json::to_string(&PingOutput {
            pong: "pong".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .unwrap_or_default()
    }

    /// 读文件,base64 返
    #[tool(name = "read_file", description = "读文件(base64 编码返)")]
    async fn read_file(&self, Parameters(args): Parameters<ReadFileArgs>) -> Result<String, ErrorData> {
        let path = expand_path(&args.path);
        let data = tokio::fs::read(&path)
            .await
            .map_err(|e| mcp_err(-32001, format!("read {} failed: {e}", path.display())))?;
        let out = ReadFileOutput {
            content_b64: base64::engine::general_purpose::STANDARD.encode(&data),
            size_bytes: data.len() as u64,
        };
        Ok(serde_json::to_string(&out).unwrap_or_default())
    }

    /// 写文件
    #[tool(name = "write_file", description = "写文件(可选 mtime 冲突检测)")]
    async fn write_file(
        &self,
        Parameters(args): Parameters<WriteFileArgs>,
    ) -> Result<String, ErrorData> {
        let path = expand_path(&args.path);
        let data = base64::engine::general_purpose::STANDARD
            .decode(&args.content_b64)
            .map_err(|e| mcp_err(-32002, format!("base64 decode failed: {e}")))?;

        // mtime 冲突检测
        if let Some(expected) = args.if_mtime_unix_ms {
            if let Ok(meta) = tokio::fs::metadata(&path).await {
                let actual = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                if actual != expected {
                    return Err(mcp_err(
                        -32003,
                        format!(
                            "mtime mismatch: expected={expected}, actual={actual} \
                             (file changed since you last read it)"
                        ),
                    ));
                }
            }
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

    /// 列目录
    #[tool(name = "list_dir", description = "列目录(可递归)")]
    async fn list_dir(&self, Parameters(args): Parameters<ListDirArgs>) -> Result<String, ErrorData> {
        let path = expand_path(&args.path);
        let mut entries = Vec::new();
        let max_depth = args.max_depth.unwrap_or(if args.recursive { 3 } else { 0 });

        fn walk(
            dir: &std::path::Path,
            entries: &mut Vec<DirEntry>,
            current_depth: usize,
            max_depth: usize,
        ) -> std::io::Result<()> {
            let read = std::fs::read_dir(dir)?;
            for e in read {
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
                    let _ = walk(&e.path(), entries, current_depth + 1, max_depth);
                }
            }
            Ok(())
        }

        walk(&path, &mut entries, 1, max_depth)
            .map_err(|e| mcp_err(-32005, format!("list {} failed: {e}", path.display())))?;

        let total = entries.len();
        Ok(serde_json::to_string(&ListDirOutput { entries, total }).unwrap_or_default())
    }

    /// 文件元信息
    #[tool(name = "stat", description = "文件元信息(存在/大小/mtime/只读)")]
    async fn stat(&self, Parameters(args): Parameters<StatArgs>) -> Result<String, ErrorData> {
        let path = expand_path(&args.path);
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

    /// git 探测
    #[tool(name = "git_status", description = "git 探测:branch / uncommitted / ahead / behind / last commit")]
    async fn git_status(
        &self,
        Parameters(args): Parameters<GitStatusArgs>,
    ) -> Result<String, ErrorData> {
        let path = expand_path(&args.path);

        // 1. 是不是 git 仓库
        let porcelain = Command::new("git")
            .args(["-C", path.to_str().unwrap_or("."), "status", "--porcelain"])
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
            Err(e) => {
                return Err(mcp_err(-32010, format!("git 启动失败:{e}")));
            }
        };

        let uncommitted = porcelain_out.lines().filter(|l| !l.is_empty()).count() as i64;

        // 2. branch
        let branch = Command::new("git")
            .args(["-C", path.to_str().unwrap_or("."), "branch", "--show-current"])
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());

        // 3. ahead / behind(以 main 为基准,失败返 0)
        let mut ahead = 0;
        let mut behind = 0;
        if let Some(ref br) = branch {
            for (arg, slot) in [("main", &mut ahead), ("HEAD", &mut behind)] {
                if arg == "main" {
                    // ahead = rev-list main..HEAD
                    if let Ok(o) = Command::new("git")
                        .args([
                            "-C",
                            path.to_str().unwrap_or("."),
                            "rev-list",
                            "--count",
                            &format!("main..{br}"),
                        ])
                        .output()
                        .await
                    {
                        if o.status.success() {
                            *slot = String::from_utf8_lossy(&o.stdout)
                                .trim()
                                .parse()
                                .unwrap_or(0);
                        }
                    }
                }
                // behind 暂不实现(需要 fetch upstream,本地不一定能跑)
            }
        }

        // 4. last commit
        let last_commit = Command::new("git")
            .args([
                "-C",
                path.to_str().unwrap_or("."),
                "rev-parse",
                "--short",
                "HEAD",
            ])
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());

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

    /// git diff
    #[tool(name = "git_diff", description = "git diff(可选 --staged)")]
    async fn git_diff(
        &self,
        Parameters(args): Parameters<GitDiffArgs>,
    ) -> Result<String, ErrorData> {
        let path = expand_path(&args.path);
        let max_bytes = args.max_bytes.unwrap_or(1024 * 1024); // 1MB

        let mut cmd = Command::new("git");
        cmd.args(["-C", path.to_str().unwrap_or("."), "diff"]);
        if args.staged {
            cmd.arg("--staged");
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let out = cmd
            .output()
            .await
            .map_err(|e| mcp_err(-32011, format!("git diff 启动失败:{e}")))?;

        if !out.status.success() {
            return Err(mcp_err(
                -32012,
                format!(
                    "git diff 失败:{}",
                    String::from_utf8_lossy(&out.stderr)
                ),
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

// ───── Main ─────

/// 2026-06-15:Panic hook 写文件(Windows release 模式不会弹窗,
/// 崩溃时这里能看到 backtrace + 错误,跟 stdout 一份)
fn install_panic_log() {
    let exe = std::env::current_exe().ok();
    let log_path = exe
        .as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.join("perspective-agent-panic.log"))
        .unwrap_or_else(|| std::path::PathBuf::from("perspective-agent-panic.log"));
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

#[tokio::main]
async fn main() -> Result<()> {
    install_panic_log();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,perspective_agent=debug")),
        )
        .init();

    info!("perspective-agent starting...");
    info!("exe path: {:?}", std::env::current_exe().ok());
    info!("cwd: {:?}", std::env::current_dir().ok());
    info!("args: {:?}", std::env::args().collect::<Vec<_>>());

    let agent = Agent::default();
    let service = StreamableHttpService::new(
        move || Ok(agent.clone()),
        LocalSessionManager::default().into(),
        Default::default(),
    );

    // 2026-06-15:绑 0.0.0.0(Y 路径),允许 LAN / 穿透访问
    // ⚠️ 当前版本**不**做鉴权 — rmcp 0.3.2 client transport 没暴露 auth_header 配置项,
    // 简单方案是依赖网络隔离(LAN / VPN / frp 自带鉴权 / 防火墙)。
    // 想加 token 鉴权得换 rmcp 版本或自己实现 MCP HTTP client(reqwest + JSON-RPC)。
    // AGENT_TOKEN env 保留语义但暂未强制校验。
    if let Ok(token) = std::env::var("AGENT_TOKEN") {
        info!("AGENT_TOKEN is set (length {}) but not enforced — v1 limitation", token.len());
    }

    let app = axum::Router::new().nest_service("/mcp", service);

    let port: u16 = std::env::var("AGENT_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(9876);
    let addr: SocketAddr = format!("0.0.0.0:{port}").parse()?;
    info!("MCP server listening on http://{addr}/mcp");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}