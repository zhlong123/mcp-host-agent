# Perspective Agent (Windows)

本机 MCP server,暴露文件 / git 工具给 Perspective server 通过 `agent://local/...` URL。

## 安装

直接运行 `perspective-agent.exe`,无依赖(纯 Rust 单文件,无 Python / Node / DLL)。

## 用法

```bash
# 默认绑 127.0.0.1:9876
perspective-agent.exe

# 自定义端口
AGENT_PORT=9999 perspective-agent.exe
```

agent 启动后会:
1. 监听 `http://127.0.0.1:9876/mcp`(MCP streamable-http)
2. 日志写到 stdout

## Perspective server 怎么连

Perspective server 跟 agent 在同一台机器时,启动 server 时自动试连 agent(端口 9876,可用 `AGENT_PORT` env 改)。

server 起来后,创建项目时 `primary_path` 填 `agent://local/C:/path/to/project` 即可。

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

## 路径约定

- 所有路径是 agent 本机路径(Windoiws 用 `C:/...` 或 `C:\...`,两种都接受)
- `~` 会被展开成 `%USERPROFILE%`

## 调试

curl 测试(server 在另一台机器或用 `127.0.0.1` 都行):

```bash
# 初始化
curl -X POST http://127.0.0.1:9876/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}'

# 调工具(read session ID from response header)
curl -X POST http://127.0.0.1:9876/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "Mcp-Session-Id: <session-id>" \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized"}'

curl -X POST http://127.0.0.1:9876/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "Mcp-Session-Id: <session-id>" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"git_status","arguments":{"path":"C:/path/to/repo"}}}'
```

## 安全

- 绑 127.0.0.1 only(本机访问,网络不可达)
- 没有任何鉴权 — 因为只听本机端口,够安全
- Y 路径会加配对码 + Bearer token,支持远端 agent

## 升级

下个版本直接覆盖 exe 即可,agent 无状态(每次启动从 DB 读,DB 在 server 那边)。

## 反馈

bug 提在 Gitea: http://localhost:8929/zhlong/perspective/issues