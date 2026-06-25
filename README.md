# MCP Host Agent

> **让 AI 优雅地「伸手」到另一台电脑——最轻量的远程 Agent 方案。**

你在 **A 电脑** 上跑 **Hermes**、**OpenClaw**、Cursor、Claude 等 MCP 客户端；在 **B 电脑** 上跑本 Agent。AI 通过标准 MCP 协议，直接在 B 上读代码、改文件、搜项目、看 Git、（可选）跑命令——**不需要** SSH 手工敲命令，不需要在 B 上再装一套 IDE，不需要把整台机器交给云端。

一个 Rust 单文件服务 + 可选桌面面板。配置好沙箱目录，复制 MCP 地址，就能用。

---

## 为什么用它

| 对比 | 传统做法 | 本 Agent |
|------|----------|----------|
| 远程改代码 | SSH + vim / 远程桌面 | MCP 工具调用，AI 直接读写 |
| 部署重量 | Docker、Node、Python 栈 | **纯 Rust 单 exe**，无运行时依赖 |
| 协议 | 各客户端各搞一套 | **标准 MCP Streamable HTTP**，Hermes / OpenClaw / Cursor 即连即用 |
| 安全 | 整机权限 | **沙箱 roots + 限额 + 可选 Token + Bash 默认关** |
| 可观测 | 黑盒 | **操作记录流** + 审计日志，改了什么看得见 |

**典型场景**

- 笔记本跑 AI，台式机 / 服务器放代码和编译环境 → Agent 跑在代码那台机器上
- 公司内网开发机，人在外网通过 VPN + Agent 远程协作
- 多台电脑各跑一个 Agent，AI 按 URL 切换「手」伸到哪台

---

## 怎么工作（30 秒看懂）

```
┌─────────────────────┐          HTTP MCP           ┌──────────────────────────┐
│  A：你的 AI 客户端   │  ────────────────────────▶  │  B：MCP Host Agent        │
│  Hermes / OpenClaw  │   http://B的IP:9876/mcp     │  mcp-host-agent-app    │
│  Cursor / Claude…   │
└─────────────────────┘                               │  ├─ 沙箱内读/写/搜/Git     │
                                                      │  ├─ 审计 + 操作记录        │
                                                      │  └─ agent.toml 配置       │
                                                      └──────────────────────────┘
```

AI 发出的每个动作（读文件、改一行、glob 搜索…）都是一次 MCP 工具调用；Agent 在 **B 电脑本地** 执行，结果回传给 A。

---

## 支持的 MCP 客户端

本 Agent 暴露 **MCP Streamable HTTP**（`/mcp`），下列客户端均已验证可对接。  
**A 电脑**跑客户端，**B 电脑**跑 Agent；工具路径填 **B 上的绝对路径**。

| 客户端 | 传输 | 文档 |
|--------|------|------|
| **[Hermes Agent](https://hermes-agent.nousresearch.com/)** | HTTP / Streamable HTTP | [MCP 配置](https://hermes-agent.nousresearch.com/docs/user-guide/features/mcp) |
| **[OpenClaw](https://openclaw.ai/)** | `streamable-http` | [MCP CLI](https://docs.openclaw.ai/cli/mcp) |
| Cursor | Streamable HTTP | Settings → MCP |
| Claude Desktop | HTTP MCP | 依版本配置 MCP Server URL |

若 B 上配置了 `token`，所有客户端均需在请求头带：`Authorization: Bearer <token>`。

### Hermes Agent

在 `~/.hermes/config.yaml`（或项目的 `mcp_servers`）中添加：

```yaml
mcp_servers:
  host_agent:
    url: "http://192.168.1.100:9876/mcp"   # B 的局域网 IP，或穿透公网 URL
    headers:
      Authorization: "Bearer 你的token"      # agent.toml 未设 token 可删掉 headers
    connect_timeout: 60
    timeout: 180
```

- 启动 Hermes 后自动发现 `read_file`、`glob`、`grep`、`git_status` 等 11 个工具
- 改配置后可执行 **`/reload-mcp`** 热加载，无需重启 Hermes
- 远程 HTTP 服务器 **无需在 A 上安装 Rust / 本仓库**，只要 URL 可达

### OpenClaw

在 `~/.openclaw/openclaw.json` 的 `mcp.servers` 中添加（需支持 `streamable-http` 的版本）：

```json
{
  "mcp": {
    "servers": {
      "host-agent": {
        "url": "http://192.168.1.100:9876/mcp",
        "transport": "streamable-http",
        "headers": {
          "Authorization": "Bearer 你的token"
        }
      }
    }
  }
}
```

或使用 CLI 一键添加：

```bash
openclaw mcp add host-agent \
  --url http://192.168.1.100:9876/mcp \
  --transport streamable-http \
  --header "Authorization=Bearer 你的token"
```

然后 **重启 OpenClaw gateway**，检查：

```bash
openclaw mcp list
openclaw tools list
```

若不想某 Agent 使用 MCP 工具，可在 profile 里 `tools.deny: ["bundle-mcp"]`（见 [OpenClaw 文档](https://docs.openclaw.ai/cli/mcp)）。

### Cursor / Claude Desktop

1. MCP 类型选 **Streamable HTTP**（或 HTTP MCP）
2. URL：`http://127.0.0.1:9876/mcp`（本机）或 `http://<B的IP>:9876/mcp`（远程）
3. 有 token 时在 Headers 填 `Authorization: Bearer ...`

### 连通性自检

| 步骤 | 命令 / 操作 |
|------|-------------|
| B 上 Agent 存活 | 浏览器或 `curl http://127.0.0.1:9876/health` |
| A 能访问 B | `curl http://<B的IP>:9876/health` |
| MCP 工具可用 | 客户端里调用 `ping`，应返回版本与 `roots` 列表 |

---

## Agent 需要做什么（部署清单）

在 **要被 AI 控制的那台电脑（B）** 上，你需要完成下面全部事项：

### 1. 安装并启动 Agent

**推荐：桌面应用**

```bash
git clone https://github.com/zhlong123/mcp-host-agent.git
cd mcp-host-agent
npm install
npm run build:app
# 运行 target/release/mcp-host-agent-app.exe
```

**或：仅 CLI（无界面）**

```bash
cargo build --release
cp agent.toml.example agent.toml
./target/release/mcp-host-agent --serve --config agent.toml
```

### 2. 配置沙箱目录 `[[roots]]`（必做）

在控制面板或 `agent.toml` 里声明 **允许 AI 访问的目录**：

```toml
[[roots]]
name = "my-project"
path = "D:/Projects/my-project"
```

- 可配置多个 root，每个有名称
- **留空 = 不限制路径**，仅适合完全可信的本机；LAN / 公网 **必须** 配置
- 工具里的 `path` 必须是 **B 电脑上的绝对路径**（Windows 用 `D:/...` 或 `D:\...`）

### 3. 网络：让 A 能连到 B

| 场景 | B 上怎么配 | A 填的 MCP 地址 |
|------|------------|-----------------|
| 同一台电脑 | 默认即可 | `http://127.0.0.1:9876/mcp` |
| 局域网 | `bind = "0.0.0.0"`，防火墙放行 9876 | `http://<B的局域网IP>:9876/mcp` |
| 跨网 / 公网 | frp、Tailscale、Cloudflare Tunnel 等暴露端口 | 隧道给出的 URL + **必须设 token** |

Windows 本机连接请用 `127.0.0.1`，不要用 `localhost`（可能走 IPv6 连失败）。

### 4. 安全项（按场景选做）

```toml
port = 9876
bind = "0.0.0.0"              # 仅本机时改为 127.0.0.1
token = "足够长的随机字符串"    # 穿透/公网：强烈建议

# allow_bash = true           # 默认 false；开启后 AI 可跑 Shell（高危）
```

- **Token**：客户端请求 `/mcp` 时需带 Bearer；不设则端口可达即可调用
- **Bash**：默认关闭；只有你需要 AI 跑 `npm test`、`cargo build` 等才打开
- **限额**：`[limits]` 控制单次读写大小、glob/grep 条数、bash 超时等（见 `agent.toml.example`）

### 5. 在 A 电脑注册 MCP Server

见上文 **[支持的 MCP 客户端](#支持的-mcp-客户端)**（含 Hermes / OpenClaw 完整配置）。通用步骤：

1. 添加 **Streamable HTTP** 类型 MCP
2. URL 填 B 的地址（公网 URL 可写在 B 的 `public_mcp_url`，控制面板里一键复制）
3. 若 B 配置了 `token`，在客户端 Headers 填 `Authorization: Bearer ...`

### 6. 验证连通

B 上访问：`http://127.0.0.1:9876/health` → 应返回 JSON `status: ok`

或在 MCP 客户端里调用 `ping` 工具，应返回版本与 roots 列表。

### 7. 改配置后重启

控制面板点 **重启**，或 CLI 重启 `--serve` 进程。保存 `agent.toml` 不会自动热加载。

---

## Agent 提供的能力（11 个 MCP 工具）

Agent **负责在 B 电脑上执行** 下列操作；AI 客户端 **负责** 决定何时调用、传什么参数。

| 工具 | Agent 做什么 | 典型用途 |
|------|--------------|----------|
| `read_file` | 读文件，文本可带行号；二进制 base64 | AI 看源码、配置、图片/PDF |
| `write_file` | 整文件新建或覆盖 | 生成新文件、大段重写 |
| `edit_file` | 精确字符串替换（须 UTF-8 文本） | 改几行代码，比整文件写更安全 |
| `list_dir` | 列目录，可递归 | 看项目结构 |
| `stat` | 文件是否存在、大小、修改时间 | 判断路径、查元信息 |
| `glob` | 按文件名模式找路径（不读内容） | `**/*.rs` 找所有 Rust 文件 |
| `grep` | 正则搜文件内容 | 找符号、搜关键字 |
| `git_status` | 分支、脏工作区、ahead/behind | AI 知道 Git 状态再改代码 |
| `git_diff` | 输出 diff，可看 staged | 改完自查 |
| `bash` | 在指定 cwd 跑 Shell（**需 `allow_bash=true`**） | 编译、测试、装依赖 |
| `ping` | 返回版本、roots、Git 是否可用 | 连通性自检 |

**Agent 不做的事：**

- 不替 AI 做推理或规划（那是客户端的事）
- 不自动扫描全盘（只在 `roots` 内响应）
- 默认不执行 Shell（除非你显式开启）
- 不提供 GUI 给 AI 点按（浏览器打开 `/` 只是说明页；管理用桌面 app）

---

## 桌面应用（B 电脑上）

| 页面 | 作用 |
|------|------|
| **控制面板** | 启停/重启 MCP、端口与 Token、沙箱目录、读写/Glob/Grep/Bash 限额、复制 MCP 地址、**Cloudflare 临时隧道**（一键公网 MCP） |
| **操作记录** | 实时流式查看 AI 每次调用了什么工具、改了哪些文件、diff 与命令输出 |

**Cloudflare 临时隧道（Quick Tunnel）：** 控制面板 → 网络 →「启动隧道」。需先启动 MCP、设置 **Bearer Token** 与 **沙箱 roots**；优先使用本机 `cloudflared`（WinGet 或自动下载）。公网 URL 每次重启会变，适合临时联调，生产环境请用 Tailscale / 自有域名隧道。

MCP 服务内嵌在桌面进程里，**不必** 再单独开一个 `mcp-host-agent.exe` 黑窗口。

---

## 配置参考（agent.toml）

```toml
port = 9876
bind = "0.0.0.0"
# token = "..."
# public_mcp_url = "http://your-tunnel.example.com/mcp"  # 控制面板展示用

[limits]
max_read_bytes = 10485760
max_write_bytes = 10485760
max_glob_results = 500
max_grep_matches = 200
max_bash_output_bytes = 1048576
bash_timeout_secs = 30

[[roots]]
name = "my-project"
path = "D:/Projects/my-project"
```

完整字段见 [agent.toml.example](agent.toml.example)。图形界面与文件二选一编辑即可。

---

## 构建要求

| 用途 | 需要 |
|------|------|
| 跑 release 二进制 | Rust 1.75+ |
| 构建桌面 app | 上述 + Node.js 18+ |

```bash
npm run build:app     # 桌面应用
cargo build --release # 仅 CLI 服务
```

---

## 安全提醒

Agent 等于 **B 电脑上的特权代理**。公网或多人环境务必：

1. 配置 `[[roots]]`，不要裸奔全盘  
2. 设置强 `token` + 隧道层鉴权  
3. 非必要不开 `allow_bash`  
4. 定期看 **操作记录** 与审计日志  

详见 [SECURITY.md](SECURITY.md)。

---

## 开发

```bash
npm run tauri dev
cargo test
```
