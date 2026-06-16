//! Glob / grep helpers scoped to allowed roots.

use crate::paths::{format_audit_path, resolve_allowed_path};
use globset::{Glob, GlobMatcher};
use regex::Regex;
use rmcp::model::ErrorData;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatch {
    pub path: String,
    pub line: u32,
    pub text: String,
}

pub fn mime_kind(path: &Path, data: &[u8]) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" => "image",
        "pdf" => "pdf",
        _ if std::str::from_utf8(data).is_ok() => "text",
        _ => "binary",
    }
}

pub fn with_line_numbers(text: &str) -> String {
    text.lines()
        .enumerate()
        .map(|(i, line)| format!("{:>6}|{}", i + 1, line))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn glob_search(
    base: &Path,
    pattern: &str,
    max_results: usize,
) -> Result<Vec<String>, String> {
    let glob = Glob::new(pattern).map_err(|e| format!("invalid glob pattern: {e}"))?;
    let matcher = glob.compile_matcher();
    let mut out = Vec::new();
    for entry in WalkDir::new(base).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if out.len() >= max_results {
            break;
        }
        let p = entry.path();
        let rel = p.strip_prefix(base).unwrap_or(p);
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if !matcher.is_match(rel_str.as_str()) {
            continue;
        }
        out.push(format_audit_path(p));
    }
    Ok(out)
}

pub fn grep_search(
    base: &Path,
    pattern: &str,
    file_glob: Option<&str>,
    max_matches: usize,
    max_read_bytes: usize,
) -> Result<Vec<GrepMatch>, String> {
    let re = Regex::new(pattern).map_err(|e| format!("invalid regex: {e}"))?;
    let file_matcher: Option<GlobMatcher> = if let Some(g) = file_glob {
        Some(
            Glob::new(g)
                .map_err(|e| format!("invalid file_glob: {e}"))?
                .compile_matcher(),
        )
    } else {
        None
    };

    let mut matches = Vec::new();
    'walk: for entry in WalkDir::new(base).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if matches.len() >= max_matches {
            break;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if let Some(m) = &file_matcher {
            let rel = path.strip_prefix(base).unwrap_or(path);
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if !m.is_match(rel_str.as_str()) {
                continue;
            }
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.len() as usize > max_read_bytes {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if matches.len() >= max_matches {
                break 'walk;
            }
            if re.is_match(line) {
                matches.push(GrepMatch {
                    path: format_audit_path(path),
                    line: (i + 1) as u32,
                    text: line.to_string(),
                });
            }
        }
    }
    Ok(matches)
}

pub fn resolve_search_base(
    user_path: &str,
    canon_roots: &[(String, PathBuf)],
) -> Result<PathBuf, ErrorData> {
    resolve_allowed_path(user_path, canon_roots)
}

pub fn format_grep_preview(matches: &[GrepMatch], truncated: bool) -> String {
    let mut lines: Vec<String> = matches
        .iter()
        .map(|m| format!("{}:{}: {}", m.path, m.line, m.text))
        .collect();
    if truncated {
        lines.push("… (truncated)".into());
    }
    if lines.is_empty() {
        "(no matches)".into()
    } else {
        lines.join("\n")
    }
}

pub fn format_glob_preview(paths: &[String], truncated: bool) -> String {
    let mut lines = paths.to_vec();
    if truncated {
        lines.push("… (truncated)".into());
    }
    if lines.is_empty() {
        "(no matches)".into()
    } else {
        lines.join("\n")
    }
}
