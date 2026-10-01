#!/usr/bin/env bash
# `just start-remote` (#358): the desktop app on this Mac, nothing compiled here.
#
#   gateway + Vite ──── Boat sandbox (scripts/remote/stack.sh)
#        ▲ forwarded to 127.0.0.1:7421 and :1420
#   desktop host ────── CI build of this working tree (scripts/remote/macos-app.sh)
#
# The gateway uses this Mac's dev credentials when it has some, so the app reads
# the tokens the gateway was provisioned with; otherwise the sandbox provisions
# them and they are copied back. The gateway's endpoint record always is.
# Quitting the app stops the remote stack.
set -euo pipefail
set -m
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${here}/../stop-job.sh"
boat() { bash "${here}/boat.sh" "$@"; }

export NESSA_STAGE=dev VITE_NESSA_STAGE=dev
namespace="$HOME/.nessa/dev"
port="$(node "${here}/../gateway-port.mjs")"

if [[ -f "${namespace}/auth/surfaces/nessa-panel.token" ]]; then
  boat put "${namespace}/auth/" .nessa/dev/auth/
  [[ -f "${namespace}/owner.token" ]] && boat put "${namespace}/owner.token" .nessa/dev/owner.token
fi

stack_pid=""
cleanup() {
  local stack="${stack_pid}"
  stack_pid=""
  stop_job "the remote gateway and UI" "${stack}"
}
trap cleanup EXIT INT TERM

# Its stdin is not the terminal: a background job that reads it, or sets its
# modes, is stopped by the terminal until it is in the foreground.
boat exec --port "${port}" --port 1420 bash scripts/remote/stack.sh </dev/null &
stack_pid=$!

# Built while the stack starts; usually already downloaded.
binary="$(bash "${here}/macos-app.sh")"

for _ in $(seq 1 240); do
  curl -sf --connect-timeout 0.5 "http://127.0.0.1:${port}/health" >/dev/null && break
  kill -0 "${stack_pid}" 2>/dev/null || { echo "→ the remote stack exited before the gateway was healthy"; exit 1; }
  sleep 0.5
done
curl -sf "http://127.0.0.1:${port}/health" >/dev/null || { echo "→ no gateway on :${port}"; exit 1; }
echo "→ remote gateway ready on :${port}"

# The app trusts the gateway its namespace's endpoint record names, checked
# against /health. The sandbox's gateway wrote that record there.
boat get .nessa/dev/logs/gateway-endpoint.json "${namespace}/logs/gateway-endpoint.json"

if [[ ! -f "${namespace}/auth/surfaces/nessa-panel.token" ]]; then
  boat get .nessa/dev/auth/ "${namespace}/auth/"
  boat get .nessa/dev/owner.token "${namespace}/owner.token"
fi

"${binary}"
