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
runner_runtime="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
transient_unit="nessa-systemd-acceptance-${GITHUB_RUN_ID:-$$}-${GITHUB_RUN_ATTEMPT:-0}"
if ! XDG_RUNTIME_DIR="$runner_runtime" systemctl --user show-environment >/dev/null 2>&1; then
  echo "hosted runner does not provide the required ephemeral user manager" >&2
  exit 1
fi

# The hosted runner's existing manager supplies only a delegated cgroup. The
# nested manager and its bus remain isolated under the temporary runtime root.
# Registration does not need linger, and CI never changes the runner account's
# linger setting.
XDG_RUNTIME_DIR="$runner_runtime" systemd-run --user --pipe --wait --collect --quiet \
  --unit="$transient_unit" \
  -p Delegate=yes -p Type=exec -d \
  -E "RUN_DIR=$runtime_directory" \
  -E "PATH=$PATH" \
  -E "CARGO_HOME=${CARGO_HOME:-$HOME/.cargo}" \
  -E "RUSTUP_HOME=${RUSTUP_HOME:-$HOME/.rustup}" \
  bash -euo pipefail <<'DELEGATED'
export XDG_RUNTIME_DIR="$RUN_DIR"
export XDG_CONFIG_HOME="$RUN_DIR/config"
export XDG_DATA_HOME="$RUN_DIR/data"
export XDG_STATE_HOME="$RUN_DIR/state"
export XDG_CACHE_HOME="$RUN_DIR/cache"
export SYSTEMD_ENVIRONMENT_GENERATOR_PATH="$RUN_DIR/empty-environment-generators"
export SYSTEMD_GENERATOR_PATH="$RUN_DIR/empty-generators"
export SYSTEMD_UNIT_PATH="$RUN_DIR/systemd/user:/usr/lib/systemd/user"
mkdir -p \
  "$XDG_CONFIG_HOME/systemd/user" \
  "$XDG_DATA_HOME" \
  "$XDG_STATE_HOME" \
  "$XDG_CACHE_HOME" \
  "$SYSTEMD_ENVIRONMENT_GENERATOR_PATH" \
  "$SYSTEMD_GENERATOR_PATH" \
  "$RUN_DIR/systemd/user"
chmod 700 \
  "$XDG_CONFIG_HOME" \
  "$XDG_CONFIG_HOME/systemd" \
  "$XDG_CONFIG_HOME/systemd/user" \
  "$XDG_DATA_HOME" \
  "$XDG_STATE_HOME" \
  "$XDG_CACHE_HOME" \
  "$SYSTEMD_ENVIRONMENT_GENERATOR_PATH" \
  "$SYSTEMD_GENERATOR_PATH" \
  "$RUN_DIR/systemd" \
  "$RUN_DIR/systemd/user"

unset DBUS_SESSION_BUS_ADDRESS
manager_log="$XDG_RUNTIME_DIR/systemd-manager.log"
bus_error="$XDG_RUNTIME_DIR/systemd-bus-error.log"
SYSTEMD_LOG_LEVEL=debug SYSTEMD_LOG_TARGET=console \
  systemd --user --unit=basic.target >"$manager_log" 2>&1 &
manager_pid=$!
stop_manager() {
  if [ -n "${manager_pid:-}" ]; then
    kill -KILL "$manager_pid" 2>/dev/null || true
    wait "$manager_pid" 2>/dev/null || true
  fi
}
trap stop_manager EXIT
print_manager_diagnostics() {
  cat "$manager_log" >&2
  if [ -s "$bus_error" ]; then
    cat "$bus_error" >&2
  fi
}

private_ready=false
for _ in $(seq 1 240); do
  if ! kill -0 "$manager_pid" 2>/dev/null; then
    set +e
    wait "$manager_pid"
    manager_status=$?
    set -e
    manager_pid=
    echo "disposable systemd user manager exited before readiness with status $manager_status" >&2
    print_manager_diagnostics
    exit 1
  fi
  if [ -S "$XDG_RUNTIME_DIR/systemd/private" ]; then
    private_ready=true
    break
  fi
  sleep 0.05
done
if [ "$private_ready" != true ]; then
  echo "disposable systemd user manager did not create its private API socket" >&2
  print_manager_diagnostics
  exit 1
fi
if ! systemctl --user start dbus.socket 2>"$bus_error"; then
  echo "disposable systemd user manager could not start its user bus" >&2
  print_manager_diagnostics
  exit 1
fi
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
bus_ready=false
for _ in $(seq 1 240); do
  if busctl --address="$DBUS_SESSION_BUS_ADDRESS" \
    status org.freedesktop.systemd1 >/dev/null 2>"$bus_error"; then
    bus_ready=true
    break
  fi
  sleep 0.05
done
if [ "$bus_ready" != true ]; then
  echo "disposable systemd user bus did not expose the manager name" >&2
  print_manager_diagnostics
  exit 1
fi
echo "disposable systemd user manager ready at $XDG_RUNTIME_DIR with $SYSTEMD_UNIT_PATH"

export NESSA_SYSTEMD_ACCEPTANCE=1
export NESSA_STAGE=dev
cargo test -p nessa-app --no-default-features \
  gateway::infrastructure::linux::reconciliation::tests::native_user_manager_is_required_for_the_linux_acceptance_gate \
  -- --ignored --exact
echo "disposable systemd gateway lifecycle completed"
DELEGATED
