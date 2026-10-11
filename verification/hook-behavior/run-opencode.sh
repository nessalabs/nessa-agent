#!/usr/bin/env bash
# Real OpenCode 1.18.31 against the scripted model. One variant loads the
# project plugin. The other adds Nessa's suppression request: OPENCODE_PURE=1,
# project config disabled, and the alternate config inputs removed.
# stdout is the summary JSON.

HOOK_BEHAVIOR_ROOT="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
source "$HOOK_BEHAVIOR_ROOT/lib.sh"

ONLY=""
OUT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --only) ONLY="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

HARNESS="opencode"
LABEL="real harness, scripted model"
mkdir -p "${HOME}/.cache/nessa-hook-behavior"
WORKDIR="$(mktemp -d "${HOME}/.cache/nessa-hook-behavior/opencode.XXXXXX")"
OUT="${OUT:-$HOOK_BEHAVIOR_ROOT/evidence/opencode}"
mkdir -p "$OUT"
ROWS="$OUT/rows.jsonl"
rm -f "$ROWS"
PREFIX="$WORKDIR/prefix"
trap 'stop_scripted_server' EXIT

echo "installing opencode-ai@1.18.31 into $PREFIX" >&2
npm install --prefix "$PREFIX" opencode-ai@1.18.31 >&2
OPENCODE="$PREFIX/node_modules/.bin/opencode"
VERSION="$("$OPENCODE" --version | head -n 1)"
start_scripted_server

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
)

write_user_config() {
  local config_home="$1"
  mkdir -p "$config_home/opencode"
  cat >"$config_home/opencode/opencode.json" <<EOF
{
  "model": "openai/gpt-4o-mini",
  "provider": {
    "openai": {
      "options": {
        "apiKey": "scripted",
        "baseURL": "http://127.0.0.1:${PORT}/v1"
      }
    }
  }
}
EOF
}

opencode_environment() {
  local project="$1" log="$2" mode="$3" suppression="$4"
  OPENCODE_ENV=(
    "HOME=$project/home"
    "XDG_CONFIG_HOME=$project/home/.config"
    "XDG_CACHE_HOME=$project/home/.cache"
    "XDG_STATE_HOME=$project/home/.state"
    "XDG_DATA_HOME=$project/home/.local/share"
    "OPENCODE_DISABLE_MODELS_FETCH=1"
    "OPENCODE_DISABLE_AUTOUPDATE=1"
    "OPENAI_API_KEY=scripted"
    "NESSA_HOOK_LOG=$log"
    "NESSA_HOOK_MODE=$mode"
  )
  if [[ "$suppression" == "on" ]]; then
    OPENCODE_ENV+=("OPENCODE_PURE=1" "OPENCODE_DISABLE_PROJECT_CONFIG=1")
  fi
}

run_opencode() {
  local project="$1" prompt="$2" stdout="$3" stderr="$4" auto="$5"
  shift 5
  local -a approve=()
  if [[ "$auto" == "yes" ]]; then
    approve=(--auto)
  fi
  set +e
  timeout 120 env -u OPENCODE_CONFIG -u OPENCODE_CONFIG_CONTENT -u OPENCODE_CONFIG_DIR \
    "$@" \
    "$OPENCODE" run --format json --model openai/gpt-4o-mini --dir "$project" \
    "${approve[@]}" "$prompt" </dev/null >"$stdout" 2>"$stderr"
  local code=$?
  set -e
  echo "$code"
}

for entry in "${ROWS_AND_MODES[@]}"; do
  row="${entry%% *}"
  mode="${entry#* }"
  if [[ -n "$ONLY" && "$ONLY" != "$row" ]]; then
    continue
  fi
  for suppression in off on; do
    project="$WORKDIR/$row-$suppression"
    sentinel="$project/sentinel"
    log="$project/hooks.jsonl"
    mkdir -p "$project/.opencode/plugin" "$sentinel" "$project/home/.config" "$project/home/.cache" "$project/home/.state" "$project/home/.local/share"
    printf '%s\n' '{"type":"module"}' >"$project/package.json"
    : >"$log"
    cp "$HOOK_BEHAVIOR_ROOT/opencode_plugin.mjs" "$project/.opencode/plugin/hook.js"
    write_user_config "$project/home/.config"
    prompt="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=${suppression} NESSA_PHASE=first NESSA_SENTINEL=${sentinel}"
    stdout="$project/stdout.txt"
    stderr="$project/stderr.txt"
    auto="yes"
    if [[ "$row" == "events-permission" ]]; then
      auto="no"
    fi
    opencode_environment "$project" "$log" "$mode" "$suppression"
    code="$(run_opencode "$project" "$prompt" "$stdout" "$stderr" "$auto" "${OPENCODE_ENV[@]}")"
    notes="Isolated home for both variants. Models fetch and autoupdate are off for both. The suppressed variant sets OPENCODE_PURE=1 and OPENCODE_DISABLE_PROJECT_CONFIG=1 and unsets OPENCODE_CONFIG, OPENCODE_CONFIG_CONTENT, and OPENCODE_CONFIG_DIR."
    if [[ "$row" == "events-resume" || "$row" == "events-compact" ]]; then
      session="$(thread_id_from "$stdout")"
      if [[ -n "$session" ]]; then
        phase="second"
        if [[ "$row" == "events-compact" ]]; then
          phase="compact"
        fi
        follow="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=${suppression} NESSA_PHASE=${phase} NESSA_SENTINEL=${sentinel} Reply with READY only."
        if [[ "$row" == "events-compact" ]]; then
          follow="/compact"
        fi
        set +e
        timeout 120 env -u OPENCODE_CONFIG -u OPENCODE_CONFIG_CONTENT -u OPENCODE_CONFIG_DIR \
          "${OPENCODE_ENV[@]}" \
          "$OPENCODE" run --format json --model openai/gpt-4o-mini --dir "$project" --auto \
          --session "$session" "$follow" </dev/null >>"$stdout" 2>>"$stderr"
        follow_code=$?
        set -e
        if [[ "$follow_code" -ne 0 && "$code" -eq 0 ]]; then
          code="$follow_code"
        fi
      else
        notes="${notes} No session id in the first run, so resume was not attempted."
      fi
    fi
    append_row "$row" "$suppression" "$code" "$log" "$stdout" "$stderr" "$sentinel" "$notes"
  done
done

python3 "$HOOK_BEHAVIOR_ROOT/evidence.py" finalize \
  --rows "$ROWS" \
  --summary "$OUT/summary.json" \
  --harness "$HARNESS" \
  --version "$VERSION" \
  --label "$LABEL"
