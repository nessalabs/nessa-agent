#!/usr/bin/env bash
# Real Codex 0.154.0 against the scripted model, with the planted hooks.json
# and again with Nessa's suppression request (features.hooks false, notify []).
# stdout is the summary JSON. The pinned package is installed into a scratch
# prefix for this run.

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

HARNESS="codex"
LABEL="real harness, scripted model"
mkdir -p "${HOME}/.cache/nessa-hook-behavior"
WORKDIR="$(mktemp -d "${HOME}/.cache/nessa-hook-behavior/codex.XXXXXX")"
OUT="${OUT:-$HOOK_BEHAVIOR_ROOT/evidence/codex}"
mkdir -p "$OUT"
ROWS="$OUT/rows.jsonl"
rm -f "$ROWS"
PREFIX="$WORKDIR/prefix"
trap 'stop_scripted_server' EXIT

echo "installing @openai/codex@0.154.0 into $PREFIX" >&2
npm install --prefix "$PREFIX" @openai/codex@0.154.0 >&2
CODEX="$PREFIX/node_modules/.bin/codex"
VERSION="$("$CODEX" --version | head -n 1)"
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

write_config() {
  local home="$1" suppression="$2"
  mkdir -p "$home"
  local hooks="true" notify=""
  if [[ "$suppression" == "on" ]]; then
    hooks="false"
    notify=$'notify = []\n'
  fi
  cat >"$home/config.toml" <<EOF
model = "scripted"
model_provider = "scripted"
check_for_update_on_startup = false
${notify}
[features]
hooks = ${hooks}

[model_providers.scripted]
name = "Scripted"
base_url = "http://127.0.0.1:${PORT}/v1"
env_key = "OPENAI_API_KEY"
wire_api = "responses"
requires_openai_auth = false
EOF
}

run_exec() {
  local home="$1" project="$2" prompt="$3" stdout="$4" stderr="$5" bypass_sandbox="$6"
  local -a sandbox=()
  if [[ "$bypass_sandbox" == "yes" ]]; then
    sandbox=(--dangerously-bypass-approvals-and-sandbox)
  fi
  set +e
  timeout 75 env \
    CODEX_HOME="$home" \
    OPENAI_API_KEY=scripted \
    "$CODEX" exec \
    --skip-git-repo-check \
    --json \
    --strict-config \
    --dangerously-bypass-hook-trust \
    "${sandbox[@]}" \
    -C "$project" \
    -m scripted \
    "$prompt" < /dev/null >"$stdout" 2>"$stderr"
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
    home="$project/codex-home"
    sentinel="$project/sentinel"
    log="$project/hooks.jsonl"
    mkdir -p "$project/.codex" "$sentinel" "$home"
    git -C "$project" init -q
    : >"$log"
    plant_command_hooks "$home/hooks.json" "$log" "$mode" "$sentinel"
    plant_command_hooks "$project/.codex/hooks.json" "$log" "$mode" "$sentinel"
    write_config "$home" "$suppression"
    prompt="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=${suppression} NESSA_PHASE=first NESSA_SENTINEL=${sentinel}"
    stdout="$project/stdout.txt"
    stderr="$project/stderr.txt"
    bypass="yes"
    if [[ "$row" == "events-permission" ]]; then
      bypass="no"
    fi
    code="$(run_exec "$home" "$project" "$prompt" "$stdout" "$stderr" "$bypass")"
    notes="features.hooks is ${suppression}; both variants bypass hook trust so the trust gate is not the difference. User hooks.json and project .codex/hooks.json are both planted."
    if [[ "$suppression" == "on" ]]; then
      notes="Nessa's request: features.hooks false and notify []. ${notes}"
    fi
    if [[ "$row" == "events-resume" || "$row" == "events-compact" ]]; then
      thread="$(thread_id_from "$stdout")"
      if [[ -n "$thread" ]]; then
        phase="second"
        if [[ "$row" == "events-compact" ]]; then
          phase="compact"
        fi
        follow="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=${suppression} NESSA_PHASE=${phase} NESSA_SENTINEL=${sentinel} Reply with READY only."
        if [[ "$row" == "events-compact" ]]; then
          follow="/compact"
        fi
        set +e
        timeout 75 env CODEX_HOME="$home" OPENAI_API_KEY=scripted \
          "$CODEX" exec \
          --skip-git-repo-check \
          --json \
          --strict-config \
          --dangerously-bypass-hook-trust \
          --dangerously-bypass-approvals-and-sandbox \
          -C "$project" \
          -m scripted \
          resume "$thread" "$follow" </dev/null >>"$stdout" 2>>"$stderr"
        follow_code=$?
        set -e
        if [[ "$follow_code" -ne 0 && "$code" -eq 0 ]]; then
          code="$follow_code"
        fi
      else
        notes="${notes} No thread id in the first exec, so resume was not attempted."
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
