# Stopping a background job and everything it started.
#
# Sourced, not run: `just start` and `scripts/run-dev-app.sh` both need this and
# both had their own copy, with the same explanation written out twice. They run
# in the same process tree — `just start` calls `just dev`, which runs the other
# — so a change to one and not the other is a teardown that half works.
#
# The group, not the pid. `set -m` puts each job in a process group of its own,
# so a plain `kill "${pid}"` reaches the recipe's own `just` and not the
# tauri/vite/app subtree beneath it — which is how an app came to outlive the
# run that started it. The bare-pid fallback is for a job that was not made a
# group leader after all; signalling a group we do not lead is not a thing to
# guess at.
stop_job() {
  local what="$1" pid="$2"
  [[ -n "${pid}" ]] || return 0
  [[ -n "${what}" ]] && echo "→ stopping ${what} (pid ${pid})"
  kill -TERM -"${pid}" 2>/dev/null || kill -TERM "${pid}" 2>/dev/null || true
  wait "${pid}" 2>/dev/null || true
}
