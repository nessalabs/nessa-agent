#!/usr/bin/env bash
# Real OpenCode 1.18.31 against the scripted model. Every row plants a project
# plugin and a user plugin under the private XDG_CONFIG_HOME. The off row sets
# neither OPENCODE_PURE nor OPENCODE_DISABLE_PROJECT_CONFIG. The on row sets
# both. suppress-pure sets only OPENCODE_PURE=1 so that flag is not the same
# cell as project-config disabling. stdout is the summary JSON.

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
trap 'stop_scripted_server; rm -rf "$WORKDIR"' EXIT

echo "installing opencode-ai@1.18.31 into $PREFIX" >&2
npm install --prefix "$PREFIX" opencode-ai@1.18.31 >&2
OPENCODE="$PREFIX/node_modules/.bin/opencode"
VERSION="$("$OPENCODE" --version | head -n 1)"
start_scripted_server

redact_paths() {
  python3 - "$WORKDIR" "$HOME" "$@" <<'PY'
import pathlib
import sys

workdir, home, *paths = sys.argv[1:]
work = workdir.encode()
home_bytes = home.encode()
for raw in paths:
    path = pathlib.Path(raw)
    if not path.is_file():
        continue
    data = path.read_bytes()
    updated = data.replace(work, b"<workdir>").replace(home_bytes, b"<home>")
    if updated != data:
        path.write_bytes(updated)
PY
}

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

plant_plugins() {
  local project="$1"
  local user_root="$project/home/.config/opencode"
  mkdir -p "$project/.opencode/plugin" "$user_root/plugin"
  printf '%s\n' '{"type":"module"}' >"$project/package.json"
  printf '%s\n' '{"type":"module"}' >"$user_root/package.json"
  sed 's/__NESSA_HOOK_SOURCE__/project/g' "$HOOK_BEHAVIOR_ROOT/opencode_plugin.mjs" \
    >"$project/.opencode/plugin/hook.js"
  sed 's/__NESSA_HOOK_SOURCE__/user/g' "$HOOK_BEHAVIOR_ROOT/opencode_plugin.mjs" \
    >"$user_root/plugin/hook.js"
}

run_case() {
  local row="$1" mode="$2" suppression="$3"
  local project="$WORKDIR/$row-$suppression"
  local sentinel="$project/sentinel"
  local log="$project/hooks.jsonl"
  mkdir -p "$sentinel" "$project/home/.config" "$project/home/.cache" "$project/home/.state" "$project/home/.local/share"
  : >"$log"
  plant_plugins "$project"
  write_user_config "$project/home/.config"
  local prompt="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=${suppression} NESSA_PHASE=first NESSA_SENTINEL=${sentinel}"
  local stdout="$project/stdout.txt"
  local stderr="$project/stderr.txt"
  local -a approve=()
  if [[ "$row" != "events-permission" ]]; then
    approve=(--auto)
  fi
  local -a flags=()
  if [[ "$suppression" == "on" ]]; then
    flags=("OPENCODE_PURE=1" "OPENCODE_DISABLE_PROJECT_CONFIG=1")
  elif [[ "$suppression" == "pure" ]]; then
    flags=("OPENCODE_PURE=1")
  fi
  local -a environment=(
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
    "${flags[@]}"
  )
  set +e
  timeout 120 env \
    -u OPENCODE_CONFIG -u OPENCODE_CONFIG_CONTENT -u OPENCODE_CONFIG_DIR \
    -u OPENCODE_PURE -u OPENCODE_DISABLE_PROJECT_CONFIG \
    "${environment[@]}" \
    "$OPENCODE" run --format json --model openai/gpt-4o-mini --dir "$project" \
    "${approve[@]}" "$prompt" </dev/null >"$stdout" 2>"$stderr"
  local code=$?
  set -e
  local notes="Project plugin and user plugin under the private XDG_CONFIG_HOME, each with its own hook_source. OPENCODE_CONFIG, OPENCODE_CONFIG_CONTENT, and OPENCODE_CONFIG_DIR are unset."
  if [[ "$suppression" == "on" ]]; then
    notes="${notes} OPENCODE_PURE=1 and OPENCODE_DISABLE_PROJECT_CONFIG=1."
  elif [[ "$suppression" == "pure" ]]; then
    notes="${notes} OPENCODE_PURE=1 only. OPENCODE_DISABLE_PROJECT_CONFIG is unset, so PURE is separate from project-config disabling."
  else
    notes="${notes} OPENCODE_PURE and OPENCODE_DISABLE_PROJECT_CONFIG are unset."
  fi
  if [[ "$row" == "events-resume" || "$row" == "events-compact" ]]; then
    local session
    session="$(thread_id_from "$stdout")"
    if [[ -n "$session" ]]; then
      local phase="second"
      if [[ "$row" == "events-compact" ]]; then
        phase="compact"
      fi
      local follow="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=${suppression} NESSA_PHASE=${phase} NESSA_SENTINEL=${sentinel} Reply with READY only."
      if [[ "$row" == "events-compact" ]]; then
        follow="/compact"
      fi
      set +e
      timeout 120 env \
        -u OPENCODE_CONFIG -u OPENCODE_CONFIG_CONTENT -u OPENCODE_CONFIG_DIR \
        -u OPENCODE_PURE -u OPENCODE_DISABLE_PROJECT_CONFIG \
        "${environment[@]}" \
        "$OPENCODE" run --format json --model openai/gpt-4o-mini --dir "$project" --auto \
        --session "$session" "$follow" </dev/null >>"$stdout" 2>>"$stderr"
      local follow_code=$?
      set -e
      if [[ "$follow_code" -ne 0 && "$code" -eq 0 ]]; then
        code="$follow_code"
      fi
    else
      notes="${notes} No session id in the first run, so resume was not attempted."
    fi
  fi
  redact_paths "$stdout" "$stderr" ${REQUESTS:+"$REQUESTS"/*}
  append_row "$row" "$suppression" "$code" "$log" "$stdout" "$stderr" "$sentinel" "$notes"
}

for entry in "${ROWS_AND_MODES[@]}"; do
  row="${entry%% *}"
  mode="${entry#* }"
  if [[ -n "$ONLY" && "$ONLY" != "$row" ]]; then
    continue
  fi
  for suppression in off on; do
    run_case "$row" "$mode" "$suppression"
  done
done

if [[ -z "$ONLY" || "$ONLY" == "suppress-pure" ]]; then
  run_case "suppress-pure" "log" "pure"
fi

python3 "$HOOK_BEHAVIOR_ROOT/evidence.py" finalize \
  --rows "$ROWS" \
  --summary "$OUT/summary.json" \
  --harness "$HARNESS" \
  --version "$VERSION" \
  --label "$LABEL"
