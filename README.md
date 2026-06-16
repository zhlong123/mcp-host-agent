# Perspective Agent

Local-first **MCP (Model Context Protocol) server** for AI coding agents: sandboxed file I/O, search, Git, and optional shell — with a Tauri desktop control panel and activity stream.

Works standalone with any MCP client over HTTP. Originally extracted from the Perspective monorepo as an independent agent crate.

**License:** [MIT](LICENSE)

## Features

- **MCP over HTTP** — default `http://127.0.0.1:9876/mcp` (streamable HTTP)
- **Path sandbox** — `[[roots]]` in `agent.toml`; only allowed directories
- **11 tools** — read / write / edit files, list & stat, glob & grep, git status & diff, ping, optional bash
- **Limits** — max read/write size, list depth, grep/glob caps, bash timeout & output size
- **Audit log** — plain-text tool call log
- **Activity stream** — JSONL + desktop UI with diff previews
- **Desktop app** — start/stop/restart MCP, edit config, copy endpoints

## Quick start

### MCP server only (CLI)

```bash
cargo build --release
cp agent.toml.example agent.toml   # edit roots & limits
./target/release/perspective-agent --serve --config agent.toml
```

Health check: `GET http://127.0.0.1:9876/health`

### Desktop app (Tauri)

```bash
npm install
npm run tauri dev      # development
npm run build:app      # release → target/release/perspective-agent-app.exe
```

## MCP tools

| Tool | Description |
|------|-------------|
| `ping` | Connectivity & roots probe |
| `read_file` | Read file (base64 + optional UTF-8 text / line numbers) |
| `write_file` | Create or overwrite file (base64) |
| `edit_file` | Exact string replace (UTF-8 text) |
| `list_dir` | List directory (optional recursive) |
| `stat` | File metadata |
| `glob` | Find paths by pattern (no content read) |
| `grep` | Regex search in file contents |
| `git_status` | Branch, dirty state, ahead/behind |
| `git_diff` | Git diff (optional staged) |
| `bash` | Shell command (**off by default**, set `allow_bash = true`) |

See [agent.toml.example](agent.toml.example) for limits and sandbox configuration.

## Connect from a remote MCP client

**Same machine:** point your client at `http://127.0.0.1:9876/mcp`.

**Another machine on LAN:**

1. Run agent on machine B (configure `bind` and `[[roots]]`).
2. Register B's URL in your MCP client, e.g. `http://192.168.1.100:9876/mcp`.
3. Use absolute paths on B when calling tools.

**Public / tunnel:** put auth at the tunnel layer **and** set `token` in `agent.toml`. Never expose an unauthenticated agent with empty roots. See [SECURITY.md](SECURITY.md).

## Build

Requires Rust 1.75+.

```bash
# Linux / macOS
cargo build --release

# Windows desktop app
npm install && npm run build:app
```

Core binary is a **single Rust executable** (no Python/Node runtime for the server).

## Configuration

Copy `agent.toml.example` → `agent.toml` (next to the binary or project root). Key fields:

- `port`, `bind`, optional `token`
- `[limits]` — byte caps, glob/grep/bash limits
- `allow_bash` — enable shell tool (high risk)
- `[[roots]]` — allowed path prefixes

## Perspective integration

When used with Perspective, the server talks to this agent via MCP JSON-RPC. Core tool names/schemas for `read_file`, `write_file`, `list_dir`, and `git_status` are the stable contract; extended tools (`edit_file`, `glob`, `grep`, `bash`) are agent-side utilities.

Project paths may use `agent://<name>/C:/path/to/project` URIs when configured in Perspective.

## Development

```bash
cargo test
npm run build:ui
npm run tauri dev
```

## Feedback

Issues and PRs welcome on GitHub.
