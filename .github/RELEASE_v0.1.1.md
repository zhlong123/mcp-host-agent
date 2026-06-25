## MCP Host Agent v0.1.1

Cloudflare 临时隧道 + 桌面 UI 稳定性修复。

### 新增

- **Cloudflare Quick Tunnel**：控制面板一键启动，自动写入 `public_mcp_url`
- **cloudflared 智能查找**：WinGet Links → 本地缓存 → 多镜像下载
- **启动脚本**：`启动 MCP Host Agent.bat`（Windows）

### 修复

- 修复隧道卡在「正在连接 Cloudflare…」（stderr 与 stdout 需并行读取）
- 修复 debug 版桌面应用误连 `localhost:1420` 导致白屏（改为始终使用打包 UI）
- Windows 探活请用 `127.0.0.1:9876/health`，勿用浏览器访问 `localhost:1420`

### Windows 附件

| 文件 | 说明 |
|------|------|
| `mcp-host-agent-app.exe` | 桌面应用（推荐，内嵌 MCP + 隧道） |
| `mcp-host-agent.exe` | 仅 CLI 服务 |

### 仓库

- GitHub：https://github.com/zhlong123/mcp-host-agent

旧名 `perspective-agent` 已弃用，请使用 `mcp-host-agent`。
