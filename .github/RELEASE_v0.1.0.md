## MCP Host Agent v0.1.0

首个公开版本。在本机运行 MCP 服务，让 **Hermes**、**OpenClaw**、Cursor 等客户端远程读写另一台电脑上的项目文件。

### 亮点

- **11 个 MCP 工具**：read / write / edit、list、stat、glob、grep、git、ping、可选 bash
- **Streamable HTTP**：`http://host:9876/mcp`，兼容 Hermes 与 OpenClaw
- **沙箱 roots** + 读写限额 + 可选 Bearer Token
- **Tauri 桌面应用**：控制面板 + 操作记录流
- **纯 Rust 核心**：CLI 单 exe，无 Node/Python 运行时依赖

### Windows 附件

| 文件 | 说明 |
|------|------|
| `mcp-host-agent-app.exe` | 桌面应用（推荐，内嵌 MCP） |
| `mcp-host-agent.exe` | 仅 CLI 服务：`mcp-host-agent.exe --serve --config agent.toml` |

### 快速开始

1. 复制 `agent.toml.example` → `agent.toml`，配置 `[[roots]]`
2. 运行 `mcp-host-agent-app.exe`，启动服务
3. 在客户端填 MCP URL：`http://127.0.0.1:9876/mcp`

详见 [README](https://github.com/zhlong123/mcp-host-agent#支持的-mcp-客户端)（含 Hermes / OpenClaw 配置示例）。

### 要求

- Windows 10+（本构建为 x86_64）
- 从源码构建其他平台：`cargo build --release` / `npm run build:app`
