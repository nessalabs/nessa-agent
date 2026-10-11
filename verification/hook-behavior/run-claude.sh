#!/usr/bin/env bash
# Pinned Claude CLI hook matrix. One isolated project per row, no
# ANTHROPIC_API_KEY, model claude-haiku-4-5-20251001. Project and user command
# hooks append to a log. --managed also plants /etc/claude-code/managed-settings.json
# and removes or restores that file afterwards. This script is not run in CI:
# it needs the Claude Max login on the machine that holds it.
# stdout is the summary JSON. The reference checks exit non-zero when they fail.

HOOK_BEHAVIOR_ROOT="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
source "$HOOK_BEHAVIOR_ROOT/lib.sh"

MANAGED=0
OUT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --managed) MANAGED=1; shift ;;
    --out) OUT="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if ! command -v claude >/dev/null 2>&1; then
  echo "claude is not on PATH" >&2
  exit 2
fi

HARNESS="claude"
LABEL="real harness, live model"
WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/nessa-claude-hooks.XXXXXX")"
OUT="${OUT:-$HOOK_BEHAVIOR_ROOT/evidence/claude}"
mkdir -p "$OUT"
ROWS="$OUT/rows.jsonl"
rm -f "$ROWS"
VERSION="$(claude --version | head -n 1)"

ROWS_AND_MODES=(
  "events-plain log"
  "events-tool log"
  "events-subagent log"
  "events-compact log"
  "events-resume log"
  "events-permission log"
  "pretool-deny deny"
  "pretool-allow-updated allow-updated"
  "pretool-ask ask"
  "pretool-defer defer"
  "pretool-continue-false continue-false"
  "prompt-additional-context context"
  "prompt-system-message system"
  "stop-decision-block stop-block"
  "fail-exit-2 exit2"
  "fail-exit-1 exit1"
  "fail-malformed malformed"
  "fail-timeout timeout"
  "suppress-settings log"
)

copy_login() {
  local dest="$1"
  mkdir -p "$dest"
  local source="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
  local name
  for name in .credentials.json credentials.json; do
    if [[ -f "$source/$name" ]]; then
      cp "$source/$name" "$dest/$name"
    fi
  done
}

run_claude() {
  local project="$1" config_dir="$2" prompt="$3" stdout="$4" stderr="$5"
  shift 5
  set +e
  timeout 180 env -u ANTHROPIC_API_KEY \
    CLAUDE_CONFIG_DIR="$config_dir" \
    claude -p "$prompt" \
    --model claude-haiku-4-5-20251001 \
    --output-format json \
    "$@" >"$stdout" 2>"$stderr"
  local code=$?
  set -e
  echo "$code"
}

prepare_row() {
  local project="$1" mode="$2"
  local config_dir="$project/user-config"
  local sentinel="$project/sentinel"
  local log="$project/hooks.jsonl"
  mkdir -p "$project/.claude" "$config_dir" "$sentinel"
  printf 'NOTES_LINE_ALPHA\n' >"$project/NOTES.txt"
  printf 'OTHER_LINE_BETA\n' >"$project/OTHER.txt"
  : >"$log"
  copy_login "$config_dir"
  plant_command_hooks "$project/.claude/settings.json" "$log" "$mode" "$sentinel"
  plant_command_hooks "$config_dir/settings.json" "$log" "$mode" "$sentinel"
}

for entry in "${ROWS_AND_MODES[@]}"; do
  row="${entry%% *}"
  mode="${entry#* }"
  project="$WORKDIR/$row"
  prepare_row "$project" "$mode"
  config_dir="$project/user-config"
  log="$project/hooks.jsonl"
  sentinel="$project/sentinel"
  stdout="$project/stdout.txt"
  stderr="$project/stderr.txt"
  suppression="off"
  extra=(--dangerously-skip-permissions)
  prompt="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=off NESSA_PHASE=first NESSA_SENTINEL=${sentinel} Reply with OK only."
  case "$row" in
    events-tool|pretool-deny|pretool-allow-updated|pretool-ask|pretool-defer|pretool-continue-false|fail-exit-2|fail-exit-1|fail-malformed|fail-timeout)
      prompt="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=off NESSA_PHASE=first NESSA_SENTINEL=${sentinel} Read NOTES.txt with the Read tool and quote its first line."
      ;;
    events-subagent)
      prompt="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=off NESSA_PHASE=first NESSA_SENTINEL=${sentinel} Use a subagent to read NOTES.txt."
      ;;
    events-permission)
      extra=()
      prompt="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=off NESSA_PHASE=first NESSA_SENTINEL=${sentinel} Run a shell command that prints PERMISSION_ROW."
      ;;
    suppress-settings)
      suppression="on"
      extra+=(--settings '{"disableAllHooks":true}')
      ;;
  esac
  code="$(run_claude "$project" "$config_dir" "$prompt" "$stdout" "$stderr" "${extra[@]}")"
  notes="Project .claude/settings.json and user settings under CLAUDE_CONFIG_DIR. ANTHROPIC_API_KEY is unset. Model is claude-haiku-4-5-20251001."
  if [[ "$row" == "events-resume" || "$row" == "events-compact" ]]; then
    session="$(thread_id_from "$stdout")"
    if [[ -n "$session" ]]; then
      follow="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=off NESSA_PHASE=second NESSA_SENTINEL=${sentinel} Reply with READY only."
      if [[ "$row" == "events-compact" ]]; then
        follow="/compact"
      fi
      set +e
      timeout 180 env -u ANTHROPIC_API_KEY CLAUDE_CONFIG_DIR="$config_dir" \
        claude -p "$follow" --resume "$session" --model claude-haiku-4-5-20251001 \
        --output-format json --dangerously-skip-permissions \
        >>"$stdout" 2>>"$stderr"
      follow_code=$?
      set -e
      if [[ "$follow_code" -ne 0 && "$code" -eq 0 ]]; then
        code="$follow_code"
      fi
    else
      notes="${notes} No session id, so resume was not attempted."
    fi
  fi
  append_row "$row" "$suppression" "$code" "$log" "$stdout" "$stderr" "$sentinel" "$notes"
done

if [[ "$MANAGED" -eq 1 ]]; then
  project="$WORKDIR/suppress-managed"
  prepare_row "$project" "log"
  managed_path="/etc/claude-code/managed-settings.json"
  backup="$WORKDIR/managed-settings.backup"
  existed=0
  if [[ -f "$managed_path" ]]; then
    sudo cp "$managed_path" "$backup"
    existed=1
  fi
  restore_managed() {
    if [[ "$existed" -eq 1 ]]; then
      sudo cp "$backup" "$managed_path"
    else
      sudo rm -f "$managed_path"
    fi
  }
  trap restore_managed EXIT
  sudo mkdir -p /etc/claude-code
  sudo cp "$project/.claude/settings.json" "$managed_path"
  code="$(run_claude "$project" "$project/user-config" \
    "NESSA_HOOK_ROW=suppress-managed NESSA_SUPPRESSION=managed NESSA_PHASE=first NESSA_SENTINEL=$project/sentinel Reply with OK only." \
    "$project/stdout.txt" "$project/stderr.txt" --dangerously-skip-permissions)"
  append_row "suppress-managed" "managed" "$code" "$project/hooks.jsonl" \
    "$project/stdout.txt" "$project/stderr.txt" "$project/sentinel" \
    "Managed hook file /etc/claude-code/managed-settings.json, restored or removed afterwards."
  restore_managed
  trap - EXIT
fi

python3 "$HOOK_BEHAVIOR_ROOT/evidence.py" finalize \
  --rows "$ROWS" \
  --summary "$OUT/summary.json" \
  --harness "$HARNESS" \
  --version "$VERSION" \
  --label "$LABEL" \
  --claude-reference
