use crate::config::RootEntry;
use rmcp::model::ErrorData;
use std::path::{Path, PathBuf};

pub fn home_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return Some(PathBuf::from(home));
        }
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if !profile.is_empty() {
            return Some(PathBuf::from(profile));
        }
    }
    None
}

/// Expand ~ → $HOME / %USERPROFILE%
pub fn expand_path(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    } else if p == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }
    PathBuf::from(p)
}

pub fn canonical_roots(roots: &[RootEntry]) -> Vec<(String, PathBuf)> {
    roots
        .iter()
        .filter_map(|r| {
            std::fs::canonicalize(&r.path)
                .ok()
                .map(|p| (r.name.clone(), p))
        })
        .collect()
}

/// Normalize a resolved path for audit log output (forward slashes, no \\?\ prefix).
pub fn format_audit_path(path: &Path) -> String {
    let mut s = path.display().to_string();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        s = stripped.to_string();
    }
    s.replace('\\', "/")
}

fn is_under_root(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

pub fn resolve_allowed_path(user_path: &str, canon_roots: &[(String, PathBuf)]) -> Result<PathBuf, ErrorData> {
    if canon_roots.is_empty() {
        return Ok(expand_path(user_path));
    }

    let expanded = expand_path(user_path);
    let resolved = resolve_canonical(&expanded).map_err(|e| mcp_err(-32020, e))?;

    for (_name, root) in canon_roots {
        if is_under_root(&resolved, root) {
            return Ok(resolved);
        }
    }

    Err(mcp_err(
        -32021,
        format!(
            "path not allowed (outside configured roots): {}",
            expanded.display()
        ),
    ))
}

fn resolve_canonical(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return std::fs::canonicalize(path).map_err(|e| format!("canonicalize {}: {e}", path.display()));
    }

    if let Some(parent) = path.parent() {
        if parent.as_os_str().is_empty() {
            return Err(format!("invalid path: {}", path.display()));
        }
        if !parent.exists() {
            return Err(format!("parent does not exist: {}", parent.display()));
        }
        let canon_parent = std::fs::canonicalize(parent)
            .map_err(|e| format!("canonicalize parent {}: {e}", parent.display()))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| format!("invalid path: {}", path.display()))?;
        return Ok(canon_parent.join(file_name));
    }

    Err(format!("invalid path: {}", path.display()))
}

pub fn mcp_err(code: i32, msg: impl Into<String>) -> ErrorData {
    ErrorData::new(rmcp::model::ErrorCode(code), msg.into(), None)
}
