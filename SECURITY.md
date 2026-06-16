# Security

Perspective Agent exposes **local file and optional shell access** to MCP clients over HTTP. Treat it as privileged infrastructure.

## Defaults

- `bash` is **disabled** unless `allow_bash = true` in `agent.toml`.
- Configure `[[roots]]` so only intended project directories are reachable.
- Prefer binding to `127.0.0.1` on single-user machines; use `0.0.0.0` only on trusted LAN/VPN.
- Set a strong `token` when exposing `/mcp` through a tunnel or public URL.

## Reporting

Open a [GitHub Issue](https://github.com/zhlong123/perspective-agent/issues) with **Security** in the title for vulnerabilities. Do not publish exploit details before a fix is available.
