//! Cloudflare Quick Tunnel sidecar (`cloudflared tunnel --url …`).

use mcp_host_agent::manager::Manager;
use parking_lot::Mutex;
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelPhase {
    Idle,
    Starting,
    Running,
    Error,
}

#[derive(Clone, Serialize)]
pub struct TunnelStatusResponse {
    pub phase: TunnelPhase,
    pub running: bool,
    pub base_url: Option<String>,
    pub mcp_url: Option<String>,
    pub message: String,
}

struct QuickTunnelInner {
    child: Option<Child>,
    phase: TunnelPhase,
    base_url: Option<String>,
    message: String,
}

pub struct QuickTunnel {
    inner: Mutex<QuickTunnelInner>,
}

impl QuickTunnel {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(QuickTunnelInner {
                child: None,
                phase: TunnelPhase::Idle,
                base_url: None,
                message: "未启动".into(),
            }),
        }
    }

    pub fn status(&self) -> TunnelStatusResponse {
        let g = self.inner.lock();
        let mcp_url = g
            .base_url
            .as_ref()
            .map(|u| format!("{}/mcp", u.trim_end_matches('/')));
        TunnelStatusResponse {
            phase: g.phase,
            running: g.phase == TunnelPhase::Running || g.phase == TunnelPhase::Starting,
            base_url: g.base_url.clone(),
            mcp_url,
            message: g.message.clone(),
        }
    }

    pub fn start(self: &Arc<Self>, manager: &Manager) -> Result<String, String> {
        {
            let g = self.inner.lock();
            if g.child.is_some() || g.phase == TunnelPhase::Starting {
                return Err("隧道已在启动或运行中".into());
            }
        }

        let cfg = manager.get_config();
        if !manager.status().online {
            return Err("请先启动 MCP 服务".into());
        }
        if cfg.token.as_ref().is_none_or(|t| t.trim().is_empty()) {
            return Err("临时隧道暴露公网，请先设置 Bearer Token 并保存".into());
        }
        if cfg.roots.is_empty() {
            return Err("请先配置沙箱目录并保存".into());
        }

        {
            let mut g = self.inner.lock();
            g.phase = TunnelPhase::Starting;
            g.base_url = None;
            g.message = "正在准备隧道（首次可能自动下载组件）…".into();
        }

        let tunnel = Arc::clone(self);
        let manager = manager.clone();
        let port = cfg.port;
        std::thread::spawn(move || {
            let cloudflared = match ensure_cloudflared(&tunnel) {
                Ok(p) => p,
                Err(e) => {
                    let mut g = tunnel.inner.lock();
                    g.phase = TunnelPhase::Error;
                    g.message = e;
                    return;
                }
            };
            if let Err(e) = run_cloudflared(&tunnel, &manager, port, &cloudflared) {
                let mut g = tunnel.inner.lock();
                g.phase = TunnelPhase::Error;
                g.message = e;
            }
        });

        Ok("正在启动 Cloudflare 临时隧道…".into())
    }

    pub fn stop(&self) -> Result<String, String> {
        let mut g = self.inner.lock();
        if let Some(mut child) = g.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        g.phase = TunnelPhase::Idle;
        g.base_url = None;
        g.message = "已停止".into();
        Ok("临时隧道已停止".into())
    }
}

fn run_cloudflared(
    tunnel: &Arc<QuickTunnel>,
    manager: &Manager,
    port: u16,
    cloudflared: &Path,
) -> Result<(), String> {
    let local = format!("http://127.0.0.1:{port}");
    let mut cmd = Command::new(cloudflared);
    cmd.args(["tunnel", "--url", &local])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 cloudflared 失败: {e}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    {
        let mut g = tunnel.inner.lock();
        g.message = "正在连接 Cloudflare…".into();
        g.child = Some(child);
    }

    let tunnel = Arc::clone(tunnel);
    let manager = manager.clone();
    let stdout_handle = stdout.map(|out| {
        let tunnel = Arc::clone(&tunnel);
        let manager = manager.clone();
        std::thread::spawn(move || {
            read_stream(out, |line| on_cloudflared_line(&tunnel, &manager, line));
        })
    });
    let stderr_handle = stderr.map(|err| {
        let tunnel = Arc::clone(&tunnel);
        let manager = manager.clone();
        std::thread::spawn(move || {
            read_stream(err, |line| on_cloudflared_line(&tunnel, &manager, line));
        })
    });
    if let Some(h) = stdout_handle {
        let _ = h.join();
    }
    if let Some(h) = stderr_handle {
        let _ = h.join();
    }

    let mut g = tunnel.inner.lock();
    match g.phase {
        TunnelPhase::Starting => {
            g.phase = TunnelPhase::Error;
            g.message = "cloudflared 已退出，未获取到 trycloudflare.com 地址".into();
        }
        TunnelPhase::Running => {
            g.phase = TunnelPhase::Idle;
            g.base_url = None;
            g.message = "隧道已断开".into();
        }
        _ => {}
    }
    g.child = None;
    Ok(())
}

fn ensure_cloudflared(tunnel: &Arc<QuickTunnel>) -> Result<PathBuf, String> {
    if let Some(p) = find_cloudflared_local() {
        return Ok(p);
    }

    {
        let mut g = tunnel.inner.lock();
        g.message = "首次使用：正在下载 cloudflared（约 50MB）…".into();
    }

    let cache = cloudflared_cache_path();
    download_cloudflared(&cache)?;
    Ok(cache)
}

/// Prefer bundled / cached / WinGet installs before hitting the network.
fn find_cloudflared_local() -> Option<PathBuf> {
    if let Some(p) = find_cloudflared_bundled() {
        return Some(p);
    }
    let cache = cloudflared_cache_path();
    if cache.is_file() {
        return Some(cache);
    }
    if let Some(p) = find_cloudflared_winget() {
        return Some(p);
    }
    if let Ok(p) = which::which("cloudflared") {
        return Some(p);
    }
    find_cloudflared_via_where()
}

#[cfg(windows)]
fn find_cloudflared_winget() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let links = PathBuf::from(&local)
        .join("Microsoft")
        .join("WinGet")
        .join("Links")
        .join("cloudflared.exe");
    if links.is_file() {
        return Some(links);
    }
    let packages = PathBuf::from(&local)
        .join("Microsoft")
        .join("WinGet")
        .join("Packages");
    let Ok(entries) = std::fs::read_dir(packages) else {
        return None;
    };
    for entry in entries.flatten() {
        let candidate = entry.path().join("cloudflared.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(not(windows))]
fn find_cloudflared_winget() -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn find_cloudflared_via_where() -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let machine = std::env::var("Path").unwrap_or_default();
    let user = std::env::var("USERPROFILE")
        .map(|p| {
            format!(
                "{};{}",
                PathBuf::from(&p)
                    .join("AppData")
                    .join("Local")
                    .join("Microsoft")
                    .join("WinGet")
                    .join("Links")
                    .display(),
                PathBuf::from(&p)
                    .join("AppData")
                    .join("Local")
                    .join("Microsoft")
                    .join("WindowsApps")
                    .display()
            )
        })
        .unwrap_or_default();
    let path_var = format!("{machine};{user}");
    let output = Command::new("cmd")
        .args(["/C", "where cloudflared"])
        .env("Path", path_var)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && Path::new(line).is_file())
        .map(PathBuf::from)
}

#[cfg(not(windows))]
fn find_cloudflared_via_where() -> Option<PathBuf> {
    None
}

fn find_cloudflared_bundled() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for name in [
        "cloudflared.exe",
        "cloudflared-x86_64-pc-windows-msvc.exe",
    ] {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn cloudflared_cache_path() -> PathBuf {
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(base)
            .join("mcp-host-agent")
            .join("cloudflared.exe");
    }
    PathBuf::from("cloudflared.exe")
}

fn download_cloudflared(dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败: {e}"))?;
    }

    let urls = cloudflared_download_urls();
    let mut errors = Vec::new();
    for (i, url) in urls.iter().enumerate() {
        {
            // best-effort status update without holding tunnel lock globally
        }
        match download_cloudflared_from_url(url, dest) {
            Ok(()) => return Ok(()),
            Err(e) => {
                errors.push(format!("源 {}: {e}", i + 1));
                let _ = std::fs::remove_file(dest.with_extension("part"));
            }
        }
    }

    Err(format!(
        "下载 cloudflared 失败（可能无法访问 GitHub）。\n\
         请在本机 PowerShell 执行: winget install Cloudflare.cloudflared\n\
         或将 cloudflared.exe 复制到:\n  {}\n\n{}",
        cloudflared_cache_path().display(),
        errors.join("\n")
    ))
}

fn download_cloudflared_from_url(url: &str, dest: &Path) -> Result<(), String> {
    let response = minreq::get(url)
        .with_timeout(120)
        .send()
        .map_err(|e| e.to_string())?;
    if response.status_code != 200 {
        return Err(format!("HTTP {}", response.status_code));
    }
    let body = response.as_bytes();
    if body.len() < 1024 * 1024 {
        return Err("响应过小，可能不是有效安装包".into());
    }

    let tmp = dest.with_extension("part");
    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| format!("写入失败: {e}"))?;
        file.write_all(body)
            .map_err(|e| format!("写入失败: {e}"))?;
    }
    std::fs::rename(&tmp, dest).map_err(|e| format!("保存 cloudflared 失败: {e}"))?;
    Ok(())
}

#[cfg(windows)]
fn cloudflared_download_urls() -> Vec<&'static str> {
    vec![
        "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe",
        "https://ghproxy.net/https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe",
        "https://mirror.ghproxy.com/https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe",
    ]
}

#[cfg(target_os = "macos")]
fn cloudflared_download_urls() -> Vec<&'static str> {
    vec![
        "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-amd64.tgz",
    ]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn cloudflared_download_urls() -> Vec<&'static str> {
    vec![
        "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64",
    ]
}

fn read_stream<R: std::io::Read + Send + 'static>(reader: R, mut on_line: impl FnMut(&str) + Send) {
    let br = BufReader::new(reader);
    for line in br.lines().map_while(Result::ok) {
        on_line(&line);
    }
}

fn on_cloudflared_line(tunnel: &QuickTunnel, manager: &Manager, line: &str) {
    let Some(base) = extract_trycloudflare_url(line) else {
        return;
    };
    let mcp_url = format!("{}/mcp", base.trim_end_matches('/'));

    {
        let mut g = tunnel.inner.lock();
        if g.base_url.as_deref() == Some(base.as_str()) {
            return;
        }
        g.phase = TunnelPhase::Running;
        g.base_url = Some(base.clone());
        g.message = format!("已连接 · {base}");
    }

    let mut c = manager.get_config();
    c.public_mcp_url = Some(mcp_url);
    if manager.set_config(c).is_ok() {
        let _ = manager.save();
    }
}

fn extract_trycloudflare_url(line: &str) -> Option<String> {
    let needle = ".trycloudflare.com";
    let idx = line.find("https://")?;
    let rest = &line[idx..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ')' || c == ']' || c == '"')
        .unwrap_or(rest.len());
    let url = rest[..end].trim();
    if url.contains(needle) {
        Some(url.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::extract_trycloudflare_url;

    #[test]
    fn parses_boxed_stderr_line() {
        let line = "2026-06-18T03:49:24Z INF |  https://raymond-rough-plus-metres.trycloudflare.com                            |";
        assert_eq!(
            extract_trycloudflare_url(line).as_deref(),
            Some("https://raymond-rough-plus-metres.trycloudflare.com")
        );
    }

    #[test]
    fn ignores_lines_without_tunnel_url() {
        assert!(extract_trycloudflare_url("2026-06-18T03:49:20Z INF Requesting new quick Tunnel").is_none());
    }
}
