//! Structured operation log for MCP tool calls (UI + JSONL persistence).

use crate::audit::CLIENT_IP;
use crate::paths::format_audit_path;
use chrono::Local;
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

const DEFAULT_CAPACITY: usize = 500;
pub const PREVIEW_MAX_LINES: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    Ping,
    Read,
    Create,
    Modify,
    List,
    Git,
    Search,
    Shell,
    Error,
}

impl OpKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::Read => "read",
            Self::Create => "create",
            Self::Modify => "modify",
            Self::List => "list",
            Self::Git => "git",
            Self::Search => "search",
            Self::Shell => "shell",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiffSummary {
    pub lines_added: u32,
    pub lines_removed: u32,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActivityDetail {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<DiffSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub id: u64,
    pub ts: String,
    pub tool: String,
    pub kind: OpKind,
    pub path: String,
    pub client_ip: String,
    pub ok: bool,
    pub summary: String,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<ActivityDetail>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ActivityQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub after_id: Option<u64>,
    pub kind: Option<OpKind>,
    pub root_prefix: Option<String>,
    pub errors_only: bool,
}

fn default_limit() -> usize {
    100
}

struct Inner {
    events: Mutex<VecDeque<ActivityEvent>>,
    next_id: AtomicU64,
    persist: Option<PathBuf>,
    capacity: usize,
    write_lock: Mutex<()>,
}

#[derive(Clone)]
pub struct ActivityLog {
    inner: Arc<Inner>,
}

impl ActivityLog {
    pub fn open(persist: Option<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Inner {
                events: Mutex::new(VecDeque::with_capacity(DEFAULT_CAPACITY)),
                next_id: AtomicU64::new(1),
                persist,
                capacity: DEFAULT_CAPACITY,
                write_lock: Mutex::new(()),
            }),
        }
    }

    pub fn begin(&self, tool: &str, path: &Path, kind: OpKind) -> ToolRecorder {
        let path_str = if path.as_os_str().is_empty() || path == Path::new("-") {
            "-".into()
        } else {
            format_audit_path(path)
        };
        let client_ip = CLIENT_IP
            .try_with(|s| s.clone())
            .unwrap_or_else(|_| "unknown".into());
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        ToolRecorder {
            log: self.clone(),
            event: ActivityEvent {
                id,
                ts: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                tool: tool.to_string(),
                kind,
                path: path_str,
                client_ip,
                ok: true,
                summary: String::new(),
                duration_ms: 0,
                detail: None,
            },
            started: Instant::now(),
            committed: false,
        }
    }

    pub async fn list(&self, query: ActivityQuery) -> Vec<ActivityEvent> {
        let events = self.inner.events.lock().await;
        let limit = query.limit.min(500);
        let mut out: Vec<ActivityEvent> = events
            .iter()
            .filter(|e| query.after_id.is_none_or(|id| e.id > id))
            .filter(|e| match query.kind {
                Some(k) => e.kind == k,
                None => true,
            })
            .filter(|e| !query.errors_only || !e.ok)
            .filter(|e| {
                query.root_prefix.as_ref().is_none_or(|prefix| {
                    e.path != "-"
                        && (e.path.starts_with(prefix)
                            || e.path.replace('\\', "/").starts_with(prefix))
                })
            })
            .cloned()
            .collect();
        if query.after_id.is_none() && out.len() > limit {
            out = out.split_off(out.len() - limit);
        }
        out
    }

    async fn push(&self, event: ActivityEvent) {
        {
            let mut events = self.inner.events.lock().await;
            events.push_back(event.clone());
            while events.len() > self.inner.capacity {
                events.pop_front();
            }
        }
        if let Some(path) = &self.inner.persist {
            let line = match serde_json::to_string(&event) {
                Ok(json) => format!("{json}\n"),
                Err(e) => {
                    tracing::warn!("activity serialize failed: {e}");
                    return;
                }
            };
            let _guard = self.inner.write_lock.lock().await;
            if let Err(e) = append_line(path, &line).await {
                tracing::warn!("activity persist failed: {e}");
            }
        }
    }
}

pub struct ToolRecorder {
    log: ActivityLog,
    event: ActivityEvent,
    started: Instant,
    committed: bool,
}

impl ToolRecorder {
    pub fn kind(&self) -> OpKind {
        self.event.kind
    }

    pub fn set_kind(&mut self, kind: OpKind) {
        self.event.kind = kind;
    }

    pub async fn ok(mut self, summary: impl Into<String>, detail: ActivityDetail) {
        self.event.ok = true;
        self.event.summary = summary.into();
        self.event.detail = Some(detail);
        self.commit().await;
    }

    pub async fn err(mut self, summary: impl Into<String>) {
        self.event.ok = false;
        self.event.kind = OpKind::Error;
        self.event.summary = summary.into();
        self.commit().await;
    }

    async fn commit(&mut self) {
        if self.committed {
            return;
        }
        self.committed = true;
        self.event.duration_ms = self.started.elapsed().as_millis() as u64;
        self.log.push(self.event.clone()).await;
    }
}

const PREVIEW_MAX_LIST: usize = 200;

pub fn preview_text_bytes(data: &[u8]) -> String {
    if data.is_empty() {
        return "(empty file)".into();
    }
    match std::str::from_utf8(data) {
        Ok(text) => {
            const MAX_CHARS: usize = 4096;
            let char_count = text.chars().count();
            if char_count <= MAX_CHARS {
                text.to_string()
            } else {
                format!(
                    "{}\n… ({} chars total, truncated)",
                    text.chars().take(MAX_CHARS).collect::<String>(),
                    char_count
                )
            }
        }
        Err(_) => format!("[binary · {} bytes]", data.len()),
    }
}

pub fn preview_dir_entries(entries: &[(String, String, u64)], truncated: bool) -> String {
    if entries.is_empty() {
        return "(empty directory)".into();
    }
    let mut lines: Vec<String> = entries
        .iter()
        .take(PREVIEW_MAX_LIST)
        .map(|(name, kind, size)| {
            let suffix = match kind.as_str() {
                "dir" => "/",
                _ => "",
            };
            let size_str = if kind == "dir" {
                String::new()
            } else {
                format!("  {size} B")
            };
            format!("{name}{suffix}{size_str}")
        })
        .collect();
    if truncated || entries.len() > PREVIEW_MAX_LIST {
        lines.push("… (truncated)".into());
    }
    lines.join("\n")
}

pub fn summarize_text_diff(old: &str, new: &str) -> DiffSummary {
    let diff = TextDiff::from_lines(old, new);
    let mut lines_added = 0u32;
    let mut lines_removed = 0u32;
    let mut preview_lines: Vec<String> = Vec::new();

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => {
                lines_added += 1;
                if preview_lines.len() < PREVIEW_MAX_LINES {
                    preview_lines.push(format!("+ {}", change.value().trim_end()));
                }
            }
            ChangeTag::Delete => {
                lines_removed += 1;
                if preview_lines.len() < PREVIEW_MAX_LINES {
                    preview_lines.push(format!("- {}", change.value().trim_end()));
                }
            }
            ChangeTag::Equal => {}
        }
    }

    let mut preview = preview_lines.join("\n");
    if lines_added + lines_removed > preview_lines.len() as u32 {
        preview.push_str("\n…");
    }

    DiffSummary {
        lines_added,
        lines_removed,
        preview,
    }
}

pub fn default_activity_log_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("mcp-host-agent-activity.jsonl")))
}

async fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(line.as_bytes()).await?;
    file.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_counts_lines() {
        let d = summarize_text_diff("a\nb\n", "a\nc\nd\n");
        assert_eq!(d.lines_removed, 1);
        assert_eq!(d.lines_added, 2);
        assert!(d.preview.contains("+ c"));
    }
}
