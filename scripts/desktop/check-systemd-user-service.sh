#!/usr/bin/env bash
set -euo pipefail

runtime_directory="$(mktemp -d)"
cleanup() {
  primary_status=$?
  cleanup_status=0
  inaccessible_directory="$runtime_directory/systemd/inaccessible/dir"
  if [ -d "$inaccessible_directory" ] && [ ! -L "$inaccessible_directory" ]; then
    chmod 700 "$inaccessible_directory" || cleanup_status=$?
  fi
  rm -rf "$runtime_directory" || cleanup_status=$?
  if [ "$cleanup_status" -ne 0 ]; then
    echo "disposable systemd runtime cleanup failed" >&2
    if [ "$primary_status" -eq 0 ]; then
      primary_status=$cleanup_status
    fi
  fi
  trap - EXIT
  exit "$primary_status"
}
trap cleanup EXIT
chmod 700 "$runtime_directory"
export XDG_RUNTIME_DIR="$runtime_directory"

# This manager exists only for this test process. The production prerequisite
# still checks logind linger separately; CI never changes the runner account's
# linger setting.
dbus-run-session -- bash -euo pipefail <<'INNER'
manager_log="$XDG_RUNTIME_DIR/systemd-manager.log"
bus_error="$XDG_RUNTIME_DIR/systemd-bus-error.log"
SYSTEMD_LOG_LEVEL=debug SYSTEMD_LOG_TARGET=console \
  systemd --user --unit=basic.target >"$manager_log" 2>&1 &
manager_pid=$!
stop_manager() {
  kill "$manager_pid" 2>/dev/null || true
  wait "$manager_pid" 2>/dev/null || true
}
trap stop_manager EXIT
print_manager_diagnostics() {
  cat "$manager_log" >&2
  if [ -s "$bus_error" ]; then
    cat "$bus_error" >&2
  fi
}

ready=false
for _ in $(seq 1 100); do
  if ! kill -0 "$manager_pid" 2>/dev/null; then
    echo "disposable systemd user manager exited before readiness" >&2
    print_manager_diagnostics
    exit 1
  fi
  if [ -S "$XDG_RUNTIME_DIR/systemd/private" ] && \
    busctl --address="$DBUS_SESSION_BUS_ADDRESS" \
      status org.freedesktop.systemd1 >/dev/null 2>"$bus_error"; then
    ready=true
    break
  fi
  sleep 0.05
done
if [ "$ready" != true ]; then
  echo "disposable systemd user manager did not acquire its D-Bus name" >&2
  print_manager_diagnostics
  exit 1
fi

export NESSA_SYSTEMD_ACCEPTANCE=1
export NESSA_STAGE=dev
cargo test -p nessa-app --no-default-features \
  gateway::infrastructure::linux::reconciliation::tests::native_user_manager_is_required_for_the_linux_acceptance_gate \
  -- --ignored --exact
INNER
