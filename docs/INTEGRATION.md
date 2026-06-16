# Perspective ↔ Perspective Agent 整合文档

> **2026-06-16 抽出独立 repo 时建档**
> 本文档是 Perspective Agent(本 repo)跟 Perspective(主 repo `zhlong/perspective`)之间"怎么连、怎么改、哪里是边界"的完整说明。
> 改任意一边前先读本文 — **唯一耦合点**(4 个 MCP tool 协议)被强类型化,改完会编译失败/运行报错,不要凭记忆动。

## 1. 关系总览

```
┌──────────────────────────────────────┐         ┌──────────────────────────────────────┐
│  Perspective (zhlong/perspective)    │         │  Perspective Agent (本 repo)         │
│  ──────────────────────────────────  │         │  ──────────────────────────────────  │
│  Rust axum 后端 (端口 8099)           │         │  Rust 单 binary (端口 9876)           │
│  React Vite 前端                      │         │  独立 repo,独立 release               │
│  SQLite (perspective.db)              │         │  无 DB(配置全从 server 那边读)        │
│  crates: core / llm / server         │         │  crates: 无(单 crate self-workspace)  │
│  ──────────────────────────────────  │         │                                      │
│  AgentClient (rmcp) ─── MCP ────────▶│ ── HTTP ──▶  Agent (rmcp ServerHandler)    │
│  4 dispatcher 函数                    │  /mcp   │  7 MCP tools                          │
│  1 RemoteAgentRegistry                │ JSON-   │  绑 0.0.0.0:9876                       │
│  1 init_agent_client                  │  RPC    │                                      │
└──────────────────────────────────────┘         └──────────────────────────────────────┘
```

**两 repo 零代码 import**:
- `crates/agent/`(原 perspective)在物理仓库已**删除**
- `perspective-agent/`(本 repo)只 import `rmcp` / `axum` / `tokio` 等通用 crate,**不** import perspective 任何代码
- 两边通信**只**通过 `http://host:port/mcp` 上的 JSON-RPC 2.0 + MCP 协议

## 2. 历史(可追溯)

| 日期 | 事件 | 关联 commit |
|---|---|---|
| 2026-06-15 | 首次新增 `crates/agent` member | `bf648d0`(原 perspective)/ `1b12d57`(原 perspective HEAD alias) |
| 2026-06-15 | 远端 agent 支持(Y 路径)— 绑 0.0.0.0 + agents DB 表 + RemoteAgentRegistry | `33e1c8e`(本 repo)/ `228ef97`(原 perspective) |
| 2026-06-15 | 调试补丁(axum TraceLayer + Windows panic log) | `2f1a6af` / `e715732` / `ea5dcfa` |
| **2026-06-16** | **抽独立 repo**:`git subtree split --prefix=crates/agent -b perspective-agent-standalone` | `bf648d0`..`2f1a6af`(6 commit)完整保留 |
| 2026-06-16 | perspective 端:`git rm -r crates/agent` + 改 test skip 模式 + web 硬编码抽 const | `dd57d03` |
| 2026-06-16 | 本 repo init + 加 `[workspace]` 头部 + 加 `.gitignore` + 加本文档 | `9a5e094` |

**抽仓库手法**:`git subtree split`(而非 `filter-branch`)。保留了原 perspective commit hash 在 message 头部,本 repo 6 个 commit 都能追溯回 perspective 仓库的具体 commit。验证方式:`git -C zhlong/perspective log --oneline -- crates/agent` 能查到原 6 个 commit。

## 3. URL scheme(perspective 端解析)

`perspective` 的 `crates/server/src/agent_dispatch.rs::ProjectRoot::parse()` 解析 `primary_path`,支持 3 种形态:

| 形态 | 路由到 |
|---|---|
| `/abs/path` 或 `file:///abs/path` | 本机 fs(`ProjectRoot::Local`,**不走 agent**) |
| `agent://local/path` | 本机 agent(`127.0.0.1:9876`,全局单例 `AGENT_CLIENT` 自动连) |
| `agent://<name>/path` | 远端 agent:查 `agents` 表拿 `url`,`RemoteAgentRegistry` 缓存连过的 `Arc<AgentClient>` |

**3 种 URL 在用户视角是统一的**:服务端把它抽象成 `ProjectRoot { Local | Agent { target, root } }`,下游 dispatcher(`git_status` / `list_files` / `read_file` / `write_file`)只调 `client_for(root)` 拿一个 `AgentClient`,不关心本机 / 远端。

## 4. 协议契约(唯一耦合点,强类型)

### 4.1 本 repo 暴露的 7 个 MCP tool

| Tool | 入参 (JSON Schema) | 返回 |
|------|------|------|
| `ping` | `{}` | `{pong: string, version: string}` |
| `read_file` | `{path: string}` | `{content_b64: string, size_bytes: number}` |
| `write_file` | `{path: string, content_b64: string, if_mtime_unix_ms?: number}` | 空(成功)/ 错误 |
| `list_dir` | `{path: string, recursive: bool, max_depth: number}` | `{entries: [{name, kind, size_bytes}], total: number}` |
| `stat` | `{path: string}` | 文件元信息 |
| `git_status` | `{path: string}` | `{is_git: bool, branch?: string, uncommitted: number, ahead: number, behind: number, last_commit?: string, error?: string}` |
| `git_diff` | `{path: string, staged?: bool}` | diff 文本 |

### 4.2 perspective server 实际调 4 个

`crates/server/src/agent_dispatch.rs` 顶部 `#[derive(Deserialize)]` struct 强类型化,改字段会**编译失败**:

- `git_status` → `GitStatusFromAgent`
- `list_dir` → `ListDirOutputFromAgent` / `DirEntryFromAgent`
- `read_file` → `ReadFileOutputFromAgent`
- `write_file` → 无返回 struct(只判 Err)

`ping` / `stat` / `git_diff` 三个是本地 utility,server 不调(留给 curl 调试 + 未来扩展)。

### 4.3 双向文件指针(改协议前查这俩文件)

- **本 repo 端 tool 定义**:`src/main.rs`,每个 `#[tool(name = "...")] async fn` 块
- **perspective 端反序列化**:`crates/server/src/agent_dispatch.rs` 顶部 4 个 struct

改任何字段名 / 类型 / 必选性,必须**同 PR 改两个文件**。

## 5. 部署形态

### 5.1 本机(server + agent 同机,最常见)

```
perspective-server (systemd user)  ──── init_agent_client(9876) ───▶  127.0.0.1:9876
                                                                       ▲
perspective-agent release binary   ──────────────────────────────────────┘
(手起,后台,绑 0.0.0.0)
```

server 启动时调 `init_agent_client(port)`,`AgentClient::try_local(9876)` 试连,连上缓存到 `AGENT_CLIENT` 全局 `OnceLock<Arc<AgentClient>>`。**没连上不 panic**,只是 server 端 `agent()` 返 None,`agent://local/...` 解析时返回 "local agent not connected"。

### 5.2 跨机(server 在 A,agent 在 B)

```
A: perspective-server
   └─ SQLite agents 表: { id, name: "laptop-b", url: "http://192.168.1.50:9876/mcp", token_encrypted, ... }
   └─ User creates project: primary_path = "agent://laptop-b/C:/Users/zhlong/proj"
   └─ /api/projects/.../files → agent_dispatch.list_files(root) → remote_registry.get_or_connect("laptop-b", url) → MCP call

B: perspective-agent
   └─ ./perspective-agent(绑 0.0.0.0:9876)
```

`RemoteAgentRegistry` 缓存连过的 `Arc<AgentClient>`,key = `name`,value = client,生命周期 = server 进程。重启 server 后重连。

### 5.3 多机(>1 远端 agent)

每个远端 agent 在 perspective 设置面板(SettingsPanel → 🛰️ 远端 Agent tab)注册一条记录,UI 上有 "测试连通" 按钮调 `/api/agents/:id/test`(本质是 `AgentClient::connect()` + `ping` tool)。多个远端 agent 互不感知,各自独立 `Arc<AgentClient>`。

## 6. 鉴权(v1 缺,边界)

### 6.1 v1 现状

- **rmcp 0.3.2 streamable-http-client transport 没暴露 auth_header 配置项**(查 rmcp issue tracker 确认,2026-06-15 验证)。
- `AGENT_TOKEN` env 在 agent 端保留语义但**不强制**。
- token 字段在 perspective DB / UI 都暂存但**未发送**到 agent。

### 6.2 v1 安全模型

靠**网络隔离**:
- 本机:绑 0.0.0.0 仍可接受,因为本机只有 perspective-server 会连
- 跨机:LAN / VPN / frp 自带鉴权 / 防火墙
- **不要直暴露公网 9876**

如果要走公网,frp / cloudflared 那层加鉴权,不要在 agent 这层加。

### 6.3 v2 路径(等 rmcp 升级)

等任一即可解锁:
- rmcp 下个 release 暴露 `StreamableHttpClientTransportConfig` 的 `auth_header` 字段 → 改 `agent_dispatch::AgentClient::connect` 加 `Authorization: Bearer <token>`,agent 端用 tower middleware 验
- 不想等 → 自己用 reqwest 实现 MCP HTTP client(mcp 协议本身不复杂,几百行)

## 7. 升级路径(改协议时)

**Step 1**:本 repo(`perspective-agent`)改 `src/main.rs` 的 tool 定义,加新字段 / 改 schema
**Step 2**:**同一个 PR**(或紧跟 commit)改 `crates/server/src/agent_dispatch.rs` 的对应 struct
**Step 3**:`cargo test -p perspective-server --test agent_dispatch_test` 跑过(4 个 e2e test 验证 `git_status` / `list_dir` / `read_file` / `write_file` 协议完整)
**Step 4**:本 repo release 改 version + perspective 端升 perspective-server 引用

**反向**也适用:server 端想加新 tool 调用(如 `stat` / `git_diff`),先在本 repo 验 tool 存在,再在 `agent_dispatch.rs` 加 dispatcher + struct + route handler 接入。

## 8. 关联文件双向索引

| 主题 | 本 repo (perspective-agent) | perspective (zhlong/perspective) |
|---|---|---|
| Cargo 入口 | `Cargo.toml`(单 crate,自 `[workspace]`) | 根 `Cargo.toml`(`members = [core, llm, server]`) |
| Tool 定义 | `src/main.rs`(7 个 `#[tool(...)]` 块) | n/a |
| MCP client 接入 | n/a | `crates/server/src/agent_dispatch.rs`(`AgentClient` + 4 dispatcher + `ProjectRoot` + `RemoteAgentRegistry`) |
| Route handler | n/a | `crates/server/src/routes.rs` 4 个 handler:`project_git_status` / `project_list_files` / `project_read_file` / `project_save_file`(都在 `if let Some(root) = ProjectRoot::parse(...).await { if root.is_agent() { ...return; } }` early-return) |
| DB schema | n/a | `crates/server/src/storage.rs` 的 `agents` 表(2026-06-15 远端 agent 引入) |
| REST 端点 | n/a | `crates/server/src/routes.rs` `/api/agents` 段(GET list / POST create / DELETE / :id/test) |
| 加密 | n/a | `crates/server/src/crypto.rs` AES-GCM-256(`PERSPECTIVE_MASTER_KEY` env / `<exe_dir>/.master_key`) |
| 集成测试 | n/a | `crates/server/tests/agent_dispatch_test.rs`(4 test,无 agent 时 `eprintln! SKIP` + return) |
| 前端 UI | n/a | `web/src/AgentsPanel.tsx`(top-level const `AGENT_BINARY_NAME`,改 binary 名只改这一处) |

## 9. 已知限制 / 悬挂(2026-06-16)

- **v1 不做 Bearer token 鉴权**(见 §6)
- **mtime 冲突检测**:`write_file` 接 `if_mtime_unix_ms` 参数但 agent 端不强制,本地 fs 模式的 mtime 检测完整保留(只在 `agent` 模式简化掉)
- **空目录不跟踪**:本 repo 没有 `icons/` 等空目录(原 perspective `crates/agent/icons/` 也是空目录,git 自然不跟踪)。如果以后加实际图标资源,要进 git
- **Windows release exe** 还没在本 repo 重新 cross compile。原 perspective 仓编译产物在 `perspective/target/x86_64-pc-windows-gnu/release/perspective-agent.exe`,ship 上 dufs `http://localhost:6990/perspective-agent-windows-x64.exe`。接管发布后,本 repo 跑 `cargo build --release --target x86_64-pc-windows-gnu` 出新产物,新路径在 `perspective-agent/target/x86_64-pc-windows-gnu/release/perspective-agent.exe`

## 10. 相关内存(会话级)

- 跟 [[perspective-project]] 强关联
- [[perspective-agent-standalone]] 是精简版项目状态(开发节奏用)
- [[gitea-as-project-backend]] 解释了 Gitea 当 git 后端的设计
- [[no-project-status-memory]] 解释了项目 memory 怎么维护
