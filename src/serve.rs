//! MCP HTTP server (default mode)

use crate::activity::{
    ActivityDetail, ActivityLog, OpKind, ToolRecorder, default_activity_log_path,
    preview_dir_entries, preview_text_bytes, summarize_text_diff, PREVIEW_MAX_LINES,
};
use crate::audit::{AuditLog, CLIENT_IP};
use crate::config::{CliArgs, RuntimeConfig, load_config, validate_config};
use crate::paths::{mcp_err, resolve_allowed_path};
use crate::search::{
    format_glob_preview, format_grep_preview, glob_search, grep_search, mime_kind, resolve_search_base,
    with_line_numbers,
};
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
use std::{future::Future, net::SocketAddr, path::{Path, PathBuf}, process::Stdio, sync::Arc};
use tokio::process::Command;
use tracing::{info, warn};

#[derive(Clone)]
struct AppState {
    config: Arc<RuntimeConfig>,
    canon_roots: Vec<(String, PathBuf)>,
    git_available: bool,
    audit: Arc<AuditLog>,
    activity: Arc<ActivityLog>,
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

    async fn resolve_path(&self, tool: &str, user_path: &str) -> Result<PathBuf, ErrorData> {
        let path = resolve_allowed_path(user_path, &self.state.canon_roots)?;
        self.state.audit.record(tool, &path).await;
        Ok(path)
    }

    fn record(&self, tool: &str, path: &Path, kind: OpKind) -> ToolRecorder {
        self.state.activity.begin(tool, path, kind)
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

    fn ensure_bash(&self) -> Result<(), ErrorData> {
        if self.state.config.allow_bash {
            Ok(())
        } else {
            Err(mcp_err(
                -32030,
                "bash disabled — set allow_bash = true in agent.toml to enable shell commands",
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
                "Perspective agent: file/git/search tools scoped to configured roots. \
                 Tools: read_file (text/image/pdf via base64), write_file, edit_file (exact replace), \
                 list_dir, stat, glob, grep, bash (requires allow_bash=true), git_status, git_diff. \
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
    /// When true, include numbered text preview for UTF-8 files (Read with line numbers).
    #[serde(default)]
    line_numbers: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ReadFileOutput {
    content_b64: String,
    size_bytes: u64,
    /// text | image | pdf | binary
    mime_kind: String,
    /// UTF-8 text preview; images/PDF/binary omit this (use content_b64).
    #[serde(skip_serializing_if = "Option::is_none")]
    content_text: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EditFileArgs {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
struct EditFileOutput {
    replacements: u32,
    bytes_written: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GlobArgs {
    /// Search root directory.
    path: String,
    /// Glob pattern, e.g. `**/*.rs` or `*.toml`.
    pattern: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct GlobOutput {
    paths: Vec<String>,
    total: usize,
    truncated: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GrepArgs {
    path: String,
    /// Rust regex syntax.
    pattern: String,
    /// Optional glob to filter files, e.g. `**/*.{rs,toml}`.
    file_glob: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct GrepMatchLine {
    path: String,
    line: u32,
    text: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct GrepOutput {
    matches: Vec<GrepMatchLine>,
    total: usize,
    truncated: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BashArgs {
    command: String,
    /// Working directory (must be under configured roots).
    #[serde(default = "default_bash_cwd")]
    cwd: String,
}

fn default_bash_cwd() -> String {
    ".".to_string()
}

#[derive(Debug, Serialize, JsonSchema)]
struct BashOutput {
    exit_code: i32,
    stdout: String,
    stderr: String,
    truncated: bool,
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
        self.state.audit.record("ping", Path::new("-")).await;
        let rec = self.record("ping", Path::new("-"), OpKind::Ping);
        let roots: Vec<String> = self
            .state
            .canon_roots
            .iter()
            .map(|(n, p)| format!("{n}={}", p.display()))
            .collect();
        let body = PingOutput {
            pong: "pong".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            git_available: self.state.git_available,
            roots: roots.clone(),
        };
        let json = serde_json::to_string(&body).unwrap_or_default();
        rec.ok(
            format!("pong · {} roots", roots.len()),
            ActivityDetail {
                result_json: Some(json.clone()),
                content_preview: Some(roots.join("\n")),
                extra: Some(format!("git={}", self.state.git_available)),
                ..Default::default()
            },
        )
        .await;
        json
    }

    #[tool(name = "read_file", description = "Read file (base64)")]
    async fn read_file(&self, Parameters(args): Parameters<ReadFileArgs>) -> Result<String, ErrorData> {
        let path = self.resolve_path("read_file", &args.path).await?;
        let rec = self.record("read_file", &path, OpKind::Read);
        let max = self.state.config.limits.max_read_bytes;
        if let Ok(meta) = tokio::fs::metadata(&path).await {
            if meta.len() as usize > max {
                rec.err(format!("file too large: {} bytes (max={max})", meta.len()))
                    .await;
                return Err(mcp_err(
                    -32007,
                    format!("file too large: {} bytes (max_read_bytes={max})", meta.len()),
                ));
            }
        }
        let data = match tokio::fs::read(&path).await {
            Ok(d) => d,
            Err(e) => {
                rec.err(format!("read failed: {e}")).await;
                return Err(mcp_err(-32001, format!("read {} failed: {e}", path.display())));
            }
        };
        if data.len() > max {
            rec.err(format!("file too large after read (max={max})")).await;
            return Err(mcp_err(-32007, format!("file too large after read (max={max})")));
        }
        let json = serde_json::to_string(&ReadFileOutput {
            content_b64: base64::engine::general_purpose::STANDARD.encode(&data),
            size_bytes: data.len() as u64,
            mime_kind: mime_kind(&path, &data).to_string(),
            content_text: std::str::from_utf8(&data).ok().map(|text| {
                if args.line_numbers {
                    with_line_numbers(text)
                } else {
                    text.to_string()
                }
            }),
        })
        .unwrap_or_default();
        let preview = std::str::from_utf8(&data)
            .ok()
            .map(|text| {
                if args.line_numbers {
                    with_line_numbers(text)
                } else {
                    preview_text_bytes(&data)
                }
            })
            .or_else(|| Some(format!("[{} · {} bytes]", mime_kind(&path, &data), data.len())));
        rec.ok(
            format!("读取 {} bytes", data.len()),
            ActivityDetail {
                bytes: Some(data.len() as u64),
                content_preview: preview,
                result_json: Some(json.clone()),
                ..Default::default()
            },
        )
        .await;
        Ok(json)
    }

    #[tool(name = "write_file", description = "Write file (base64); mtime param accepted but not enforced in agent mode")]
    async fn write_file(
        &self,
        Parameters(args): Parameters<WriteFileArgs>,
    ) -> Result<String, ErrorData> {
        let path = self.resolve_path("write_file", &args.path).await?;
        let old_text = if path.exists() {
            tokio::fs::read(&path).await.ok().and_then(|b| String::from_utf8(b).ok())
        } else {
            None
        };
        let kind = if old_text.is_some() {
            OpKind::Modify
        } else {
            OpKind::Create
        };
        let rec = self.record("write_file", &path, kind);
        let data = match base64::engine::general_purpose::STANDARD.decode(&args.content_b64) {
            Ok(d) => d,
            Err(e) => {
                rec.err(format!("base64 decode: {e}")).await;
                return Err(mcp_err(-32002, format!("base64 decode failed: {e}")));
            }
        };
        let max = self.state.config.limits.max_write_bytes;
        if data.len() > max {
            rec.err(format!("payload too large: {} bytes", data.len())).await;
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
        if let Err(e) = tokio::fs::write(&path, &data).await {
            rec.err(format!("write failed: {e}")).await;
            return Err(mcp_err(-32004, format!("write {} failed: {e}", path.display())));
        }
        let new_mtime = tokio::fs::metadata(&path)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let json = serde_json::to_string(&WriteFileOutput {
            new_mtime_unix_ms: new_mtime,
            bytes_written: data.len(),
        })
        .unwrap_or_default();
        let new_text = String::from_utf8_lossy(&data);
        let diff = old_text
            .as_ref()
            .map(|old| summarize_text_diff(old, new_text.as_ref()))
            .filter(|d| d.lines_added > 0 || d.lines_removed > 0);
        let summary = match kind {
            OpKind::Create => format!("新建 {} bytes", data.len()),
            _ => match &diff {
                Some(d) => format!("修改 +{} -{} 行", d.lines_added, d.lines_removed),
                None => format!("覆盖 {} bytes", data.len()),
            },
        };
        let content_preview = if diff.is_none() {
            Some(preview_text_bytes(&data))
        } else {
            None
        };
        rec.ok(
            summary,
            ActivityDetail {
                bytes: Some(data.len() as u64),
                content_preview,
                diff,
                result_json: Some(json.clone()),
                ..Default::default()
            },
        )
        .await;
        Ok(json)
    }

    #[tool(name = "list_dir", description = "List directory (optional recursive)")]
    async fn list_dir(&self, Parameters(args): Parameters<ListDirArgs>) -> Result<String, ErrorData> {
        let path = self.resolve_path("list_dir", &args.path).await?;
        let rec = self.record("list_dir", &path, OpKind::List);
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

        if let Err(e) = walk(&path, &mut entries, 1, max_depth, max_entries, &mut truncated) {
            rec.err(format!("list failed: {e}")).await;
            return Err(mcp_err(-32005, format!("list {} failed: {e}", path.display())));
        }
        let total = entries.len();
        let listing: Vec<(String, String, u64)> = entries
            .iter()
            .map(|e| (e.name.clone(), e.kind.clone(), e.size_bytes))
            .collect();
        let json = serde_json::to_string(&ListDirOutput {
            entries,
            total,
            truncated,
        })
        .unwrap_or_default();
        let summary = if truncated {
            format!("列出 {total} 项 (已截断)")
        } else {
            format!("列出 {total} 项")
        };
        rec.ok(
            summary,
            ActivityDetail {
                content_preview: Some(preview_dir_entries(&listing, truncated)),
                extra: Some(format!("recursive={} depth={max_depth}", args.recursive)),
                result_json: Some(json.clone()),
                ..Default::default()
            },
        )
        .await;
        Ok(json)
    }

    #[tool(name = "stat", description = "File metadata")]
    async fn stat(&self, Parameters(args): Parameters<StatArgs>) -> Result<String, ErrorData> {
        let path = self.resolve_path("stat", &args.path).await?;
        let rec = self.record("stat", &path, OpKind::Read);
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
                let json = serde_json::to_string(&StatOutput {
                    exists: true,
                    kind: Some(kind.to_string()),
                    size_bytes: Some(meta.len()),
                    mtime_unix_ms: mtime,
                    is_readonly: Some(meta.permissions().readonly()),
                })
                .unwrap_or_default();
                let preview = format!(
                    "kind: {kind}\nsize: {} bytes\nmtime_ms: {}\nreadonly: {}",
                    meta.len(),
                    mtime.map(|m| m.to_string()).unwrap_or_else(|| "-".into()),
                    meta.permissions().readonly()
                );
                rec.ok(
                    format!("{kind} · {} bytes", meta.len()),
                    ActivityDetail {
                        bytes: Some(meta.len()),
                        content_preview: Some(preview),
                        result_json: Some(json.clone()),
                        ..Default::default()
                    },
                )
                .await;
                Ok(json)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let json = serde_json::to_string(&StatOutput {
                    exists: false,
                    kind: None,
                    size_bytes: None,
                    mtime_unix_ms: None,
                    is_readonly: None,
                })
                .unwrap_or_default();
                rec.ok(
                    "不存在",
                    ActivityDetail {
                        content_preview: Some("exists: false".into()),
                        result_json: Some(json.clone()),
                        ..Default::default()
                    },
                )
                .await;
                Ok(json)
            }
            Err(e) => {
                rec.err(format!("stat failed: {e}")).await;
                Err(mcp_err(-32006, format!("stat {} failed: {e}", path.display())))
            }
        }
    }

    #[tool(name = "git_status", description = "Git status: branch / uncommitted / ahead / behind (upstream) / last commit")]
    async fn git_status(
        &self,
        Parameters(args): Parameters<GitStatusArgs>,
    ) -> Result<String, ErrorData> {
        self.ensure_git()?;
        let path = self.resolve_path("git_status", &args.path).await?;
        let rec = self.record("git_status", &path, OpKind::Git);
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
                let err = String::from_utf8_lossy(&o.stderr).to_string();
                let json = serde_json::to_string(&GitStatusOutput {
                    is_git: false,
                    branch: None,
                    uncommitted: 0,
                    ahead: 0,
                    behind: 0,
                    last_commit: None,
                    error: Some(err.clone()),
                })
                .unwrap_or_default();
                rec.ok(
                    "非 git 仓库",
                    ActivityDetail {
                        result_json: Some(json.clone()),
                        extra: Some(err),
                        ..Default::default()
                    },
                )
                .await;
                return Ok(json);
            }
            Err(e) => {
                rec.err(format!("git start failed: {e}")).await;
                return Err(mcp_err(-32010, format!("git failed to start: {e}")));
            }
        };
        let uncommitted = porcelain_out.lines().filter(|l| !l.is_empty()).count() as i64;
        let branch = git_stdout(&["-C", repo, "branch", "--show-current"]).await;
        let (ahead, behind) = git_ahead_behind(repo).await;
        let last_commit = git_stdout(&["-C", repo, "rev-parse", "--short", "HEAD"]).await;
        let json = serde_json::to_string(&GitStatusOutput {
            is_git: true,
            branch: branch.clone(),
            uncommitted,
            ahead,
            behind,
            last_commit: last_commit.clone(),
            error: None,
        })
        .unwrap_or_default();
        let mut preview = format!(
            "branch: {}\nuncommitted: {}\nahead: {} · behind: {}\nHEAD: {}",
            branch.as_deref().unwrap_or("-"),
            uncommitted,
            ahead,
            behind,
            last_commit.as_deref().unwrap_or("-"),
        );
        let porcelain_trim = porcelain_out.trim();
        if !porcelain_trim.is_empty() {
            preview.push_str("\n---\n");
            preview.push_str(porcelain_trim);
        }
        rec.ok(
            format!(
                "{} · {} 未提交 · ahead {} behind {}",
                branch.as_deref().unwrap_or("?"),
                uncommitted,
                ahead,
                behind
            ),
            ActivityDetail {
                content_preview: Some(preview),
                result_json: Some(json.clone()),
                extra: last_commit,
                ..Default::default()
            },
        )
        .await;
        Ok(json)
    }

    #[tool(name = "git_diff", description = "Git diff (optional --staged)")]
    async fn git_diff(
        &self,
        Parameters(args): Parameters<GitDiffArgs>,
    ) -> Result<String, ErrorData> {
        self.ensure_git()?;
        let path = self.resolve_path("git_diff", &args.path).await?;
        let rec = self.record("git_diff", &path, OpKind::Git);
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
        let out = match cmd.output().await {
            Ok(o) => o,
            Err(e) => {
                rec.err(format!("git diff start: {e}")).await;
                return Err(mcp_err(-32011, format!("git diff failed to start: {e}")));
            }
        };
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).to_string();
            rec.err(err.clone()).await;
            return Err(mcp_err(-32012, format!("git diff failed: {err}")));
        }
        let full = String::from_utf8_lossy(&out.stdout).to_string();
        let (truncated, body) = if full.len() > max_bytes {
            (true, full[..max_bytes].to_string())
        } else {
            (false, full.clone())
        };
        let line_count = body.lines().filter(|l| !l.is_empty()).count();
        let json = serde_json::to_string(&GitDiffOutput {
            bytes: body.len(),
            truncated,
            diff: body.clone(),
        })
        .unwrap_or_default();
        let preview: String = body.lines().take(PREVIEW_MAX_LINES).collect::<Vec<_>>().join("\n");
        let content_preview = if body.is_empty() {
            Some("(no diff)".into())
        } else if truncated {
            Some(format!("{preview}\n… (truncated)"))
        } else {
            Some(body.clone())
        };
        rec.ok(
            format!(
                "{}{} · {} 行",
                if args.staged { "staged " } else { "" },
                if truncated { "diff (截断)" } else { "diff" },
                line_count
            ),
            ActivityDetail {
                bytes: Some(body.len() as u64),
                content_preview,
                result_json: Some(json.clone()),
                diff: if preview.is_empty() {
                    None
                } else {
                    Some(crate::activity::DiffSummary {
                        lines_added: line_count as u32,
                        lines_removed: 0,
                        preview,
                    })
                },
                ..Default::default()
            },
        )
        .await;
        Ok(json)
    }

    #[tool(
        name = "edit_file",
        description = "Edit file by exact string replace (Read first). UTF-8 text files only."
    )]
    async fn edit_file(
        &self,
        Parameters(args): Parameters<EditFileArgs>,
    ) -> Result<String, ErrorData> {
        let path = self.resolve_path("edit_file", &args.path).await?;
        let rec = self.record("edit_file", &path, OpKind::Modify);
        let data = tokio::fs::read(&path).await.map_err(|e| {
            mcp_err(-32001, format!("read {} failed: {e}", path.display()))
        })?;
        let old_text = String::from_utf8(data.clone())
            .map_err(|_| mcp_err(-32014, "edit_file requires UTF-8 text file"))?;
        if args.old_string.is_empty() {
            rec.err("old_string must not be empty").await;
            return Err(mcp_err(-32015, "old_string must not be empty"));
        }
        let count = old_text.matches(&args.old_string).count();
        if count == 0 {
            rec.err("old_string not found").await;
            return Err(mcp_err(-32016, "old_string not found in file"));
        }
        if count > 1 && !args.replace_all {
            rec.err(format!("old_string occurs {count} times; set replace_all=true"))
                .await;
            return Err(mcp_err(
                -32017,
                format!("old_string occurs {count} times — set replace_all=true or use a unique snippet"),
            ));
        }
        let new_text = if args.replace_all {
            old_text.replace(&args.old_string, &args.new_string)
        } else {
            old_text.replacen(&args.old_string, &args.new_string, 1)
        };
        let replacements = if args.replace_all { count as u32 } else { 1 };
        let new_bytes = new_text.as_bytes();
        if new_bytes.len() > self.state.config.limits.max_write_bytes {
            rec.err("result too large").await;
            return Err(mcp_err(-32008, "edited file exceeds max_write_bytes"));
        }
        tokio::fs::write(&path, new_bytes)
            .await
            .map_err(|e| mcp_err(-32004, format!("write {} failed: {e}", path.display())))?;
        let diff = summarize_text_diff(&old_text, &new_text);
        let json = serde_json::to_string(&EditFileOutput {
            replacements,
            bytes_written: new_bytes.len(),
        })
        .unwrap_or_default();
        rec.ok(
            format!("edit · {replacements} replacement(s)"),
            ActivityDetail {
                bytes: Some(new_bytes.len() as u64),
                content_preview: Some(format!(
                    "replacements: {replacements}\n---\n{}",
                    preview_text_bytes(new_bytes)
                )),
                diff: Some(diff),
                result_json: Some(json.clone()),
                ..Default::default()
            },
        )
        .await;
        Ok(json)
    }

    #[tool(
        name = "glob",
        description = "Find files by glob pattern under path (paths only, does not read content)"
    )]
    async fn glob(&self, Parameters(args): Parameters<GlobArgs>) -> Result<String, ErrorData> {
        let base = resolve_search_base(&args.path, &self.state.canon_roots)?;
        self.state.audit.record("glob", &base).await;
        let rec = self.record("glob", &base, OpKind::Search);
        let max = self.state.config.limits.max_glob_results;
        let paths = match glob_search(&base, &args.pattern, max + 1) {
            Ok(p) => p,
            Err(e) => {
                rec.err(e.clone()).await;
                return Err(mcp_err(-32022, e));
            }
        };
        let truncated = paths.len() > max;
        let paths: Vec<String> = paths.into_iter().take(max).collect();
        let total = paths.len();
        let preview = format_glob_preview(&paths, truncated);
        let json = serde_json::to_string(&GlobOutput {
            total,
            truncated,
            paths: paths.clone(),
        })
        .unwrap_or_default();
        rec.ok(
            format!("glob `{0}` · {total} path(s)", args.pattern),
            ActivityDetail {
                content_preview: Some(preview),
                result_json: Some(json.clone()),
                extra: Some(args.pattern.clone()),
                ..Default::default()
            },
        )
        .await;
        Ok(json)
    }

    #[tool(
        name = "grep",
        description = "Search file contents with regex under path (optional file_glob filter)"
    )]
    async fn grep(&self, Parameters(args): Parameters<GrepArgs>) -> Result<String, ErrorData> {
        let base = resolve_search_base(&args.path, &self.state.canon_roots)?;
        self.state.audit.record("grep", &base).await;
        let rec = self.record("grep", &base, OpKind::Search);
        let max = self.state.config.limits.max_grep_matches;
        let max_read = self.state.config.limits.max_read_bytes;
        let raw = match grep_search(
            &base,
            &args.pattern,
            args.file_glob.as_deref(),
            max + 1,
            max_read,
        ) {
            Ok(m) => m,
            Err(e) => {
                rec.err(e.clone()).await;
                return Err(mcp_err(-32023, e));
            }
        };
        let truncated = raw.len() > max;
        let preview = format_grep_preview(&raw.iter().take(max).cloned().collect::<Vec<_>>(), truncated);
        let matches: Vec<GrepMatchLine> = raw
            .into_iter()
            .take(max)
            .map(|m| GrepMatchLine {
                path: m.path,
                line: m.line,
                text: m.text,
            })
            .collect();
        let total = matches.len();
        let json = serde_json::to_string(&GrepOutput {
            total,
            truncated,
            matches: matches.clone(),
        })
        .unwrap_or_default();
        rec.ok(
            format!("grep `{0}` · {total} match(es)", args.pattern),
            ActivityDetail {
                content_preview: Some(preview),
                result_json: Some(json.clone()),
                extra: args.file_glob.clone(),
                ..Default::default()
            },
        )
        .await;
        Ok(json)
    }

    #[tool(
        name = "bash",
        description = "Run shell command (high risk). Requires allow_bash=true in agent.toml. cwd must be under roots."
    )]
    async fn bash(&self, Parameters(args): Parameters<BashArgs>) -> Result<String, ErrorData> {
        self.ensure_bash()?;
        let cwd = self.resolve_path("bash", &args.cwd).await?;
        let rec = self.record("bash", &cwd, OpKind::Shell);
        let timeout = std::time::Duration::from_secs(self.state.config.limits.bash_timeout_secs);
        let max_out = self.state.config.limits.max_bash_output_bytes;

        #[cfg(windows)]
        let mut cmd = Command::new("cmd");
        #[cfg(windows)]
        cmd.args(["/C", &args.command]);

        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.args(["-c", &args.command]);
            c
        };

        cmd.current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let out = match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                rec.err(format!("spawn failed: {e}")).await;
                return Err(mcp_err(-32031, format!("bash failed to start: {e}")));
            }
            Err(_) => {
                rec.err("timeout").await;
                return Err(mcp_err(-32032, "bash command timed out"));
            }
        };

        let exit_code = out.status.code().unwrap_or(-1);
        let (truncated, stdout) = truncate_utf8(&out.stdout, max_out);
        let (stderr_trunc, stderr) = truncate_utf8(&out.stderr, max_out);
        let any_trunc = truncated || stderr_trunc;

        let json = serde_json::to_string(&BashOutput {
            exit_code,
            stdout: stdout.clone(),
            stderr: stderr.clone(),
            truncated: any_trunc,
        })
        .unwrap_or_default();

        let mut preview = format!("$ {}\n[cwd: {}]\nexit: {exit_code}", args.command, cwd.display());
        if !stdout.is_empty() {
            preview.push_str("\n--- stdout ---\n");
            preview.push_str(&stdout);
        }
        if !stderr.is_empty() {
            preview.push_str("\n--- stderr ---\n");
            preview.push_str(&stderr);
        }
        if any_trunc {
            preview.push_str("\n… (output truncated)");
        }

        rec.ok(
            format!("bash exit={exit_code}"),
            ActivityDetail {
                content_preview: Some(preview),
                result_json: Some(json.clone()),
                extra: Some(args.command.clone()),
                ..Default::default()
            },
        )
        .await;
        Ok(json)
    }
}

fn truncate_utf8(bytes: &[u8], max: usize) -> (bool, String) {
    if bytes.len() <= max {
        return (false, String::from_utf8_lossy(bytes).into_owned());
    }
    (true, String::from_utf8_lossy(&bytes[..max]).into_owned())
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
    run_with_shutdown(cli, rx, None).await
}

pub async fn run_with_shutdown(
    cli: CliArgs,
    mut shutdown: tokio::sync::watch::Receiver<()>,
    activity: Option<Arc<ActivityLog>>,
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

    let activity_path = runtime.activity_log.clone().or_else(default_activity_log_path);
    if let Some(p) = &activity_path {
        info!("activity log: {}", p.display());
    }
    let activity = activity.unwrap_or_else(|| Arc::new(ActivityLog::open(activity_path)));

    let runtime_cfg = Arc::new(runtime);
    let state = Arc::new(AppState {
        config: Arc::clone(&runtime_cfg),
        canon_roots,
        git_available,
        audit: AuditLog::new(audit_path),
        activity,
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
