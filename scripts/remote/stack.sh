#!/usr/bin/env bash
# Runs on the build sandbox for `just start-remote` (#358): the gateway and the
# dev UI server that the desktop host on the Mac connects to through forwarded
# ports. When either exits, or the session that started this ends, both stop.
set -euo pipefail
set -m
source "$(dirname "${BASH_SOURCE[0]}")/../stop-job.sh"

server_pid=""
web_pid=""
cleanup() {
  local web="${web_pid}" server="${server_pid}"
  web_pid=""
  server_pid=""
  stop_job "the dev UI server" "${web}"
  stop_job "nessa-server" "${server}"
}
trap cleanup EXIT INT TERM HUP

# server:run writes the dev agent config once, naming the nessa MCP server only
# if it is already built (scripts/dev-agent-config.mjs).
cargo build -p nessa-mcp
pnpm server:run &
server_pid=$!
# The same command `tauri dev` runs before it opens a window.
pnpm dev &
web_pid=$!
wait -n "${server_pid}" "${web_pid}"
