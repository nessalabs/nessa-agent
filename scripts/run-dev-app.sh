#!/usr/bin/env bash
# Run `tauri dev` so the app cannot outlive this command.
#
# `tauri dev` starts vite and then the app binary beneath it, and `just` runs
# recipes under job control, which puts that subtree in a process group of its
# own. A signal sent to the recipe therefore lands on the recipe and stops
# there: the tree above the app is torn down, the app is not, and what is left
# is a window still painted from memory with no vite serving its modules and no
# gateway connection ever attempted. It looks like an app that will not connect.
#
# So the subtree is started as a job whose group is known, and the trap takes
# that group down. Scoping to the group, rather than to the app's path, is
# deliberate: worktrees symlink `target/` to the main checkout, so every
# checkout's app resolves to the same binary and killing by path would let one
# worktree stop another's running app.
#
# A SIGKILL to this script still orphans the app — nothing a trap can do about
# that — but every ordinary exit, Ctrl-C and SIGTERM now takes it with us.
set -euo pipefail
set -m

# The same rule `just start` uses, and it calls this script, so the two must not
# drift.
source "$(dirname "${BASH_SOURCE[0]}")/stop-job.sh"

app_pid=""
cleanup() {
  local app="${app_pid}"
  app_pid=""
  stop_job "" "${app}"
}
trap cleanup EXIT INT TERM

# Free :1420 before the app is started, not while it is starting.
#
# `tauri dev` waits for the dev server to answer before it opens the window,
# and `pnpm dev` frees the port as its own first step. Run in that order, the
# wait is satisfied by the *previous* run's vite — still serving, about to be
# killed — so the window opens, loads its modules from that server, and is left
# pointing at a dead one the moment the port is freed. Nothing retries: what
# the person gets is a black rectangle and no error, and the app looks broken
# when only its page was pulled out from under it.
#
# Freeing here makes the port genuinely empty before anything probes it, so the
# wait can only be satisfied by the vite this run started.
node "$(dirname "${BASH_SOURCE[0]}")/free-dev-port.mjs"

pnpm app &
app_pid=$!
wait "${app_pid}"
