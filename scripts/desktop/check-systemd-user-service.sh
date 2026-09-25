#!/usr/bin/env bash
set -euo pipefail

runtime_directory="$(mktemp -d)"
cleanup() {
  find "$runtime_directory" -depth -delete
}
trap cleanup EXIT
chmod 700 "$runtime_directory"
export XDG_RUNTIME_DIR="$runtime_directory"

# This manager exists only for this test process. The production prerequisite
# still checks logind linger separately; CI never changes the runner account's
# linger setting.
dbus-run-session -- bash -euo pipefail <<'INNER'
systemd --user --unit=basic.target &
manager_pid=$!
stop_manager() {
  kill "$manager_pid" 2>/dev/null || true
  wait "$manager_pid" 2>/dev/null || true
}
trap stop_manager EXIT

ready=false
for _ in $(seq 1 100); do
  if busctl --user status org.freedesktop.systemd1 >/dev/null 2>&1; then
    ready=true
    break
  fi
  sleep 0.05
done
if [ "$ready" != true ]; then
  echo "disposable systemd user manager did not acquire its D-Bus name" >&2
  exit 1
fi

export NESSA_SYSTEMD_ACCEPTANCE=1
export NESSA_STAGE=dev
cargo test -p nessa-app --no-default-features \
  gateway::infrastructure::linux::reconciliation::tests::native_user_manager_is_required_for_the_linux_acceptance_gate \
  -- --ignored --exact
INNER
