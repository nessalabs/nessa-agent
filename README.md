# Nessa

A desktop and gateway app for running AI coding agents across local, SSH, and paired machines.

Nessa gives you a fast floating chat panel (macOS menu bar or Linux floating window) backed by a local WebSocket gateway, Cedar authorization, and MCP tool/skill servers.

## What's inside

- **Desktop panel** — Tauri 2 + React 19 floating chat surface with tabs, Markdown composer, and frosted or clear modes.
- **Gateway (`nessa server`)** — owns conversations, agent sessions, workspaces, environments, and Cedar policy.
- **CLI & MCP (`nessa call`, `nessa mcp`)** — drive Nessa from scripts or external agents over MCP (with built-in SEP-2640 skills).
- **SDK (`nessa-sdk`)** — reusable Rust contracts for agent execution, MCP apps, and MCP skills.

## Quick start

Requires Node 22+, `pnpm`, Rust 1.89+, and [`just`](https://just.systems).

```bash
pnpm install
just dev
```

`just dev` starts the local gateway (provisioning local dev credentials automatically) and opens the desktop panel. To install the local Claude ACP harness once:

```bash
(cd crates/nessa-sdk/harnesses/claude-acp && npm ci --omit=dev)
```

## Useful commands

| Command | What it does |
| --- | --- |
| `just dev` | Start local gateway and desktop app |
| `just server` | Start local gateway only |
| `just web` | Run the UI in a browser without Tauri |
| `just release` | Build a shipping desktop bundle |
| `pnpm check` | Run formatting, lint, and test suites |

## Learn more

- [Architecture](docs/ARCHITECTURE.md)
- [Codebase structure](docs/codebase-structure.md)
- [System state atlas](docs/state/README.md)
- [Protocol & MCP catalog](protocol/README.md)
- [Contributor guide](AGENTS.md)
