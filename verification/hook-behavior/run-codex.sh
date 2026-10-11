#!/usr/bin/env bash
# Real Codex 0.154.0 against the scripted model. config.toml leaves hooks
# enabled and does not set notify. Suppression on adds
# `codex exec -c features.hooks=false -c 'notify=[]'` on the first exec and on
# resume. That is the hooks portion of Nessa's thread/start override, not the
# full CODEX_CONFIG document. stdout is the summary JSON. The pinned package
# is installed into a scratch prefix for this run.

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

# A partial run must not replace the committed evidence directory.
if [[ -n "$ONLY" && -z "$OUT" ]]; then
  echo "--only needs --out" >&2
  exit 2
fi
if [[ -n "$ONLY" ]]; then
  committed="$(realpath -m "$HOOK_BEHAVIOR_ROOT/evidence/codex")"
  if [[ "$(realpath -m "$OUT")" == "$committed" ]]; then
    echo "--only cannot write to the committed evidence directory" >&2
    exit 2
  fi
fi
if [[ -n "$ONLY" ]]; then
  known=0
  for entry in "${ROWS_AND_MODES[@]}"; do
    if [[ "$ONLY" == "${entry%% *}" ]]; then
      known=1
      break
    fi
  done
  if [[ "$known" -eq 0 ]]; then
    echo "unknown row: $ONLY" >&2
    exit 2
  fi
fi

HARNESS="codex"
LABEL="real harness, scripted model"
mkdir -p "${HOME}/.cache/nessa-hook-behavior"
WORKDIR="$(mktemp -d "${HOME}/.cache/nessa-hook-behavior/codex.XXXXXX")"
OUT="${OUT:-$HOOK_BEHAVIOR_ROOT/evidence/codex}"
mkdir -p "$OUT"
ROWS="$WORKDIR/rows.jsonl"
SUMMARY="$WORKDIR/summary.json"
: >"$ROWS"
PREFIX="$WORKDIR/prefix"
trap 'stop_scripted_server; rm -rf "$WORKDIR"' EXIT

echo "installing @openai/codex@0.154.0 into $PREFIX" >&2
npm install --prefix "$PREFIX" @openai/codex@0.154.0 >&2
CODEX="$PREFIX/node_modules/.bin/codex"
VERSION="$("$CODEX" --version | head -n 1)"
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

write_config() {
  local home="$1"
  mkdir -p "$home"
  cat >"$home/config.toml" <<EOF
model = "scripted"
model_provider = "scripted"
check_for_update_on_startup = false

[features]
hooks = true

[model_providers.scripted]
name = "Scripted"
base_url = "http://127.0.0.1:${PORT}/v1"
env_key = "OPENAI_API_KEY"
wire_api = "responses"
requires_openai_auth = false
EOF
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
    plant_command_hooks "$home/hooks.json" "$log" "$mode" "$sentinel" user
    plant_command_hooks "$project/.codex/hooks.json" "$log" "$mode" "$sentinel" project
    write_config "$home"
    prompt="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=${suppression} NESSA_PHASE=first NESSA_SENTINEL=${sentinel}"
    stdout="$project/stdout.txt"
    stderr="$project/stderr.txt"
    override=()
    sandbox=(--dangerously-bypass-approvals-and-sandbox)
    if [[ "$row" == "events-permission" ]]; then
      sandbox=()
    fi
    if [[ "$suppression" == "on" ]]; then
      override=(-c features.hooks=false -c 'notify=[]')
    fi
    set +e
    timeout 75 env \
      CODEX_HOME="$home" \
      OPENAI_API_KEY=scripted \
      "$CODEX" exec \
      "${override[@]}" \
      --skip-git-repo-check \
      --json \
      --strict-config \
      --dangerously-bypass-hook-trust \
      "${sandbox[@]}" \
      -C "$project" \
      -m scripted \
      "$prompt" < /dev/null >"$stdout" 2>"$stderr"
    code=$?
    set -e
    notes="config.toml leaves hooks enabled and does not set notify. Both variants bypass hook trust. User hooks.json is tagged --source user and project .codex/hooks.json is tagged --source project."
    if [[ "$suppression" == "on" ]]; then
      notes="Suppression on passes -c features.hooks=false -c notify=[] on exec and on resume. That is the hooks portion of Nessa's thread/start override, not the full CODEX_CONFIG document. ${notes}"
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
          "${override[@]}" \
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
        echo "no session id: ${row} ${suppression}" >&2
        if [[ "$code" -eq 0 ]]; then
          code=1
        fi
      fi
    fi
    redact_paths "$stdout" "$stderr" ${REQUESTS:+"$REQUESTS"/*}
    append_row "$row" "$suppression" "$code" "$log" "$stdout" "$stderr" "$sentinel" "$notes"
  done
done

set +e
python3 "$HOOK_BEHAVIOR_ROOT/evidence.py" finalize \
  --rows "$ROWS" \
  --summary "$SUMMARY" \
  --harness "$HARNESS" \
  --version "$VERSION" \
  --label "$LABEL"
finalize_status=$?
set -e
if [[ "$finalize_status" -eq 0 ]]; then
  publish_snapshot "$OUT" "$ROWS" "$SUMMARY"
fi
exit "$finalize_status"
