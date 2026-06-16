# 本机 MCP Agent

在本机运行 **MCP（Model Context Protocol）服务**，供 Cursor、Claude Desktop 等 MCP 客户端通过 HTTP 调用：读改文件、搜索、Git、可选 Shell。带 Tauri 桌面控制面板与操作记录页。

**License:** [MIT](LICENSE)

## 环境要求

- Rust 1.75+
- 桌面应用构建还需 Node.js 18+

## 安装与运行

### 方式一：桌面应用（推荐）

```bash
git clone https://github.com/zhlong123/perspective-agent.git
cd perspective-agent
npm install
npm run build:app
```

产物：`target/release/perspective-agent-app.exe`（Windows）或对应平台的 app 二进制。

启动后：

1. 在控制面板配置 **沙箱目录**、端口、限额
2. 点击 **保存配置** → **启动服务**（或 **重启**）
3. 复制 **本机 MCP** 地址，填到 MCP 客户端

### 方式二：仅 CLI 服务

```bash
cargo build --release
cp agent.toml.example agent.toml   # 编辑 roots 与 limits
./target/release/perspective-agent --serve --config agent.toml
```

健康检查：

```bash
curl http://127.0.0.1:9876/health
```

## 连接 MCP 客户端

| 场景 | MCP 地址 |
|------|----------|
| 本机 | `http://127.0.0.1:9876/mcp` |
| 局域网其他机器 | `http://<本机IP>:9876/mcp` |
| 穿透/公网 | 隧道暴露后的 URL，并在 `agent.toml` 配置 `token` |

Windows 建议用 `127.0.0.1`，避免 `localhost` 走 IPv6 连不上。

在 MCP 客户端中添加 **Streamable HTTP** 类型的 MCP Server，填入上述地址。调用工具时使用 **本机绝对路径**（如 `D:/Projects/foo/src/main.rs`）。

## 配置（agent.toml）

复制 `agent.toml.example` 为 `agent.toml`（与 exe 同目录或项目根），主要项：

```toml
port = 9876
bind = "0.0.0.0"          # 仅本机用时改为 127.0.0.1
# token = "随机长字符串"   # 穿透/公网时建议开启

[limits]
max_read_bytes = 10485760
max_write_bytes = 10485760
# … 见 agent.toml.example

# allow_bash = true        # 默认关闭，开启后可跑 shell

[[roots]]
name = "my-project"
path = "D:/Projects/my-project"
```

- **roots**：只允许访问列出的目录；留空则不限路径（公网场景危险）
- 修改配置后需 **重启 MCP** 生效

也可在桌面 **控制面板** 图形编辑并保存。

## MCP 工具

| 工具 | 说明 |
|------|------|
| `read_file` | 读文件（支持文本行号） |
| `write_file` | 新建/覆盖文件 |
| `edit_file` | 字符串精准替换 |
| `list_dir` | 列目录（可递归） |
| `stat` | 文件元信息 |
| `glob` | 按文件名模式搜索 |
| `grep` | 正则搜索文件内容 |
| `git_status` | Git 状态 |
| `git_diff` | Git diff |
| `bash` | 执行 Shell（需 `allow_bash = true`） |
| `ping` | 连通性与沙箱探测 |

## 桌面应用

| 页面 | 功能 |
|------|------|
| 控制面板 | 启停/重启 MCP、网络与限额、沙箱目录、保存配置 |
| 操作记录 | 流式查看每次工具调用、diff 与输出预览 |

## 开发

```bash
npm run tauri dev      # 桌面应用热重载
cargo test             # Rust 单元测试
npm run build:ui       # 仅构建前端
```

## 安全

详见 [SECURITY.md](SECURITY.md)。公网暴露务必配置 `token` 与 `[[roots]]`，默认不要开启 `allow_bash`。

## 问题反馈

https://github.com/zhlong123/perspective-agent/issues
