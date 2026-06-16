# Security

本服务通过 HTTP 向 MCP 客户端暴露**本机文件与可选 Shell 能力**，请按特权服务对待。

## 建议

- `bash` 默认关闭，仅在 `agent.toml` 中设置 `allow_bash = true` 后可用。
- 配置 `[[roots]]`，仅开放需要的项目目录。
- 单机使用优先 `bind = "127.0.0.1"`；仅在可信 LAN/VPN 下使用 `0.0.0.0`。
- 穿透或公网访问时设置强 `token`，并在隧道层再加鉴权。

## 报告漏洞

在 [GitHub Issues](https://github.com/zhlong123/mcp-host-agent/issues) 标题注明 **Security**，勿公开利用细节。
