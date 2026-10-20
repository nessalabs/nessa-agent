#!/usr/bin/env bash
# Claude Code cloud sessions prepare the same tools Cloud setup does
# (scripts/cloud-startup.sh owns the list), then hand its environment to the
# session's shell. Local sessions are left alone. A session with several
# repositories never loads this file; the cloud environment's setup script
# covers those (docs/codebase-structure.md, "Cloud development preparation").
set -euo pipefail
[ "${CLAUDE_CODE_REMOTE:-}" = true ] || exit 0
repository=${CLAUDE_PROJECT_DIR:?}
cloud_root=${NESSA_CLOUD_ROOT:-/opt/nessa-cloud}
log=$(mktemp -t nessa-cloud-startup.XXXXXX.log)
# Whatever this prints on stdout reaches the session as context: keep the
# installer's output in the log and say only how it went.
if bash "$repository/scripts/cloud-startup.sh" setup "$repository" > "$log" 2>&1; then
  if [ -n "${CLAUDE_ENV_FILE:-}" ]; then cat "$cloud_root/env.sh" >> "$CLAUDE_ENV_FILE"; fi
  echo "Cloud tools prepared (scripts/cloud-startup.sh): $(tail -n 1 "$log")"
  grep -F 'WebKit checks cannot run' "$log" || true
else
  status=$?
  echo "Cloud tool preparation failed (exit $status); its output is in $log:"
  tail -n 20 "$log"
fi
