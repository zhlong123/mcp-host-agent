# Perspective Agent (Windows / Linux / macOS)

> **Status: standalone**(2026-06-16 抽出)
> 原是 [Perspective](http://localhost:8929/zhlong/perspective) 仓库的 `crates/agent` member(2026-06-15 ship 远端 Desktop Agent 后判定零耦合,拆出独立 repo)。
> 历史通过 `git subtree split` 完整保留,6 commits 追溯可查。
>
> **Backward contract**:与 Perspective server 之间只通过 MCP JSON-RPC over HTTP 通信,**4 个 tool 协议名 + schema 是唯一耦合**(见下面"工具协议契约"段)。改协议必须双边同 PR。

本机 MCP server,暴露文件 / git 工具给 Perspective server 通过 `agent://local/...` 或 `agent://<name>/...` URL。

## Build

需要 Rust 1.75+ 和 cargo。

### Linux / macOS(本机)

```bash
cargo build --release
# 产物:target/release/perspective-agent
```

### Windows(mingw cross from Linux,zhlong 当前用的链路)

```bash
# 装一次 target
rustup target add x86_64-pc-windows-gnu

# 需要 mingw-w64(Arch: pacman -S mingw-w64-gcc,Debian/Ubuntu: apt install gcc-mingw-w64-x86-64)
cargo build --release --target x86_64-pc-windows-gnu
# 产物:target/x86_64-pc-windows-gnu/release/perspective-agent.exe
```

产物在 `target/<target-triple>/release/perspective-agent[.exe]`,**纯 Rust 单文件,无 Python / Node / DLL 依赖**。

### 桌面管理界面 (Tauri)

图形界面用于配置 roots、启停 MCP、查看审计日志; MCP 服务仍由 `perspective-agent.exe --serve` sidecar 提供。

```bash
npm install
npm run tauri dev      # 开发
npx tauri build        # 产物: target/release/perspective-agent-app.exe
```

## 装/跑

直接运行二进制:

```bash
# 默认绑 0.0.0.0:9876(允许 LAN / 穿透访问)
./perspective-agent           # Linux/macOS
perspective-agent.exe         # Windows

# 自定义端口
AGENT_PORT=9999 ./perspective-agent
```

agent 启动后会:
1. 监听 `http://0.0.0.0:9876/mcp`(MCP streamable-http)
2. 日志写到 stdout(console subsystem,跑起来会弹黑窗口 — 不影响,服务在)

## Perspective server 怎么连

Perspective server 跟 agent 在**同一台机器**时,启动 server 时自动试连 agent(端口 9876,可用 `AGENT_PORT` env 改)。

跨机(server 在主机 A,agent 在 LAN 机器 B):
1. 在 B 上跑 agent(默认 0.0.0.0)
2. 在 A 上 Perspective 设置 → 远端 Agent → 新增 Agent:填 name + B 的 URL(http://192.168.1.100:9876/mcp)
3. 创建项目时 `primary_path` 填 `agent://<name>/C:/path/to/project`

⚠️ **v1 不做 Bearer token 鉴权** — rmcp 0.3.2 streamable-http-client transport 没暴露 auth_header 配置。靠网络隔离(LAN / VPN / frp 自带鉴权 / 防火墙)。如果要走公网,在 frp / cloudflared 那层加鉴权,不要直接暴露 9876。

## 工具清单

agent 暴露 7 个 MCP 工具:

| 工具 | 用途 |
|---|---|
| `ping` | 连通性测试 |
| `read_file` | 读文件(base64) |
| `write_file` | 写文件(可选 mtime 冲突检测) |
| `list_dir` | 列目录(支持递归) |
| `stat` | 文件元信息 |
| `git_status` | git 探测(branch / uncommitted / ahead / last_commit) |
| `git_diff` | git diff(可选 --staged) |

## 工具协议契约(与 Perspective server 唯一耦合)

改本表任意字段必须**同 PR 改 server 端 `crates/server/src/agent_dispatch.rs`**。

| Tool | 入参 | 返回 |
|------|------|------|
| `git_status` | `{path}` | `{is_git, branch?, uncommitted, ahead, behind, last_commit?, error?}` |
| `list_dir` | `{path, recursive, max_depth}` | `{entries: [{name, kind, size_bytes}], total}` |
| `read_file` | `{path}` | `{content_b64, size_bytes}` |
| `write_file` | `{path, content_b64, if_mtime_unix_ms?}` | `null`/空 |

`ping` / `stat` / `git_diff` 三个工具是本地 utility,server 不调。

## 路径约定

- 所有路径是 agent 本机路径(Windows 用 `C:/...` 或 `C:\...`,两种都接受;Linux/macOS 用 `/abs/path`)
- `~` 会被展开成 `%USERPROFILE%`(Windows)/ `$HOME`(Linux/macOS)

## 调试

curl 测试(server 在另一台机器或 `127.0.0.1` 都行):

```bash
# 1. 初始化(会拿 session ID)
curl -X POST http://127.0.0.1:9876/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}'

# 2. initialized 通知(带 session)
curl -X POST http://127.0.0.1:9876/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "Mcp-Session-Id: <session-id>" \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized"}'

# 3. 调工具
curl -X POST http://127.0.0.1:9876/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "Mcp-Session-Id: <session-id>" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"git_status","arguments":{"path":"C:/path/to/repo"}}}'
```

## 升级

下个版本直接覆盖 binary 即可,agent 无状态(每次启动从 server 那边 DB 读配置)。

## 反馈

bug 提在 Gitea: http://localhost:8929/zhlong/perspective-agent/issues
