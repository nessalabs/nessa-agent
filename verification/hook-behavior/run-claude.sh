#!/usr/bin/env bash
# Claude hook matrix for surveyed CLI 2.1.257. The script reads
# `claude --version` before any run and exits unless that version is 2.1.257.
# --allow-claude-version <v> allows v and records it in the summary.
# One isolated project per row. Every claude
# process starts in that project, so the repo checkout's .claude/settings.json
# is not the file that loads. ANTHROPIC_API_KEY is unset. The model is
# claude-haiku-4-5-20251001.
#
# CLAUDE_CONFIG_DIR is the real login directory. Credential files are not
# copied, and this script does not write into that directory. The Claude CLI
# writes session transcripts into that login directory, under projects/. This
# script reads those transcripts and does not delete them. The user hook is a
# settings file inside the row project, passed with --settings. Project hooks
# live in the project's .claude/settings.json. Each command carries
# `hook_cmd.py --source project|user|managed` so the log names the source and
# Claude does not merge the commands.
#
# --dangerously-skip-permissions is passed only on those in-project
# invocations. events-permission omits it.
#
# --managed plants /etc/claude-code/managed-settings.json for two rows and
# restores or removes it before publishing evidence. If that restore fails,
# nothing is published, the backup and work directory are kept, and the script
# exits non-zero. This script
# is not run in CI: it needs the Claude Max login on the machine that holds it.
# stdout is the summary JSON. The reference checks exit non-zero when they fail.

HOOK_BEHAVIOR_ROOT="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
source "$HOOK_BEHAVIOR_ROOT/lib.sh"

SURVEYED_CLAUDE_VERSION="2.1.257"
MANAGED=0
OUT=""
ALLOW_CLAUDE_VERSION=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --managed) MANAGED=1; shift ;;
    --out) OUT="$2"; shift 2 ;;
    --allow-claude-version)
      if [[ $# -lt 2 || -z "$2" ]]; then
        echo "--allow-claude-version needs a version" >&2
        exit 2
      fi
      ALLOW_CLAUDE_VERSION="$2"
      shift 2
      ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if ! command -v claude >/dev/null 2>&1; then
  echo "claude is not on PATH" >&2
  exit 2
fi

# Read the version before creating the work directory or the evidence
# directory. The first token of `claude --version` is the release, for
# example 2.1.257 from "2.1.257 (Claude Code)".
version_dir="$(mktemp -d "${TMPDIR:-/tmp}/nessa-claude-version.XXXXXX")"
version_status=0
VERSION="$(cd "$version_dir" && claude --version | head -n 1)" || version_status=$?
rm -rf "$version_dir"
if [[ "$version_status" -ne 0 || -z "${VERSION:-}" ]]; then
  echo "claude --version failed" >&2
  exit 2
fi
claude_version="${VERSION%% *}"
expected_claude_version="$SURVEYED_CLAUDE_VERSION"
if [[ -n "$ALLOW_CLAUDE_VERSION" ]]; then
  expected_claude_version="$ALLOW_CLAUDE_VERSION"
fi
if [[ "$claude_version" != "$expected_claude_version" ]]; then
  if [[ -n "$ALLOW_CLAUDE_VERSION" ]]; then
    echo "claude version is ${claude_version}, --allow-claude-version is ${ALLOW_CLAUDE_VERSION}" >&2
  else
    echo "claude version is ${claude_version}, surveyed version is ${SURVEYED_CLAUDE_VERSION}" >&2
  fi
  exit 2
fi

HARNESS="claude"
LABEL="real harness, live model"
LOGIN_DIR="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
TRANSCRIPT_DIR="$LOGIN_DIR"
WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/nessa-claude-hooks.XXXXXX")"
MANAGED_PATH="/etc/claude-code/managed-settings.json"
MANAGED_BACKUP="$WORKDIR/managed-settings.backup"
MANAGED_EXISTED=0
MANAGED_PLANTED=0
KEEP_WORKDIR=0

# Put the managed file back. On success MANAGED_PLANTED is 0. On failure the
# backup is still in the work directory and the caller must not publish.
restore_managed_settings() {
  local restore_status=0
  if [[ "$MANAGED_PLANTED" -ne 1 ]]; then
    return 0
  fi
  if [[ "$MANAGED_EXISTED" -eq 1 ]]; then
    sudo cp "$MANAGED_BACKUP" "$MANAGED_PATH" || restore_status=$?
    if [[ "$restore_status" -ne 0 ]]; then
      echo "error: failed to restore $MANAGED_PATH from $MANAGED_BACKUP" >&2
      return "$restore_status"
    fi
  else
    sudo rm -f "$MANAGED_PATH" || restore_status=$?
    if [[ "$restore_status" -ne 0 ]]; then
      echo "error: failed to remove $MANAGED_PATH" >&2
      return "$restore_status"
    fi
  fi
  MANAGED_PLANTED=0
}

# A failed restore leaves the work directory in place. The backup lives there.
cleanup() {
  local status=$?
  if [[ "$KEEP_WORKDIR" -eq 1 ]]; then
    exit "$status"
  fi
  if [[ "$MANAGED_PLANTED" -eq 1 ]]; then
    local restore_status=0
    restore_managed_settings || restore_status=$?
    if [[ "$restore_status" -ne 0 ]]; then
      exit "$restore_status"
    fi
  fi
  rm -rf "$WORKDIR"
  exit "$status"
}
trap cleanup EXIT

OUT="${OUT:-$HOOK_BEHAVIOR_ROOT/evidence/claude}"
mkdir -p "$OUT"
ROWS="$WORKDIR/rows.jsonl"
SUMMARY="$WORKDIR/summary.json"
: >"$ROWS"

# Stored Claude fields only. A work directory under $HOME would otherwise
# land in result and permission_denials. /tmp paths are left as they are.
redact_home_fields() {
  python3 - "$HOME" "$1" <<'PY'
import json
import pathlib
import sys

home, raw = sys.argv[1:]
if len(home) <= 1:
    raise SystemExit(0)
path = pathlib.Path(raw)
if not path.is_file():
    raise SystemExit(0)
text = path.read_text(encoding="utf-8", errors="replace")
decoder = json.JSONDecoder()
values = []
index = 0
length = len(text)
while index < length:
    while index < length and text[index].isspace():
        index += 1
    if index >= length:
        break
    try:
        value, end = decoder.raw_decode(text, index)
    except json.JSONDecodeError:
        break
    index = end
    values.append(value)

def rewrite(value):
    if isinstance(value, str):
        return value.replace(home, "<home>")
    if isinstance(value, list):
        return [rewrite(item) for item in value]
    if isinstance(value, dict):
        return {key: rewrite(item) for key, item in value.items()}
    return value

changed = False
for value in values:
    if not isinstance(value, dict):
        continue
    result = value.get("result")
    if isinstance(result, str) and home in result:
        value["result"] = result.replace(home, "<home>")
        changed = True
    denials = value.get("permission_denials")
    if isinstance(denials, list):
        updated = rewrite(denials)
        if updated != denials:
            value["permission_denials"] = updated
            changed = True
if changed:
    path.write_text(
        "".join(json.dumps(value, ensure_ascii=False) + "\n" for value in values),
        encoding="utf-8",
    )
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
  "suppress-settings log"
  "control-user log"
)

invoke_claude() {
  local project="$1" prompt="$2" stdout="$3" stderr="$4" redirect="$5"
  shift 5
  set +e
  if [[ "$redirect" == "append" ]]; then
    (
      cd "$project" && timeout 180 env -u ANTHROPIC_API_KEY \
        CLAUDE_CONFIG_DIR="$LOGIN_DIR" \
        claude -p "$prompt" \
        --model claude-haiku-4-5-20251001 \
        --output-format json \
        "$@"
    ) >>"$stdout" 2>>"$stderr"
  else
    (
      cd "$project" && timeout 180 env -u ANTHROPIC_API_KEY \
        CLAUDE_CONFIG_DIR="$LOGIN_DIR" \
        claude -p "$prompt" \
        --model claude-haiku-4-5-20251001 \
        --output-format json \
        "$@"
    ) >"$stdout" 2>"$stderr"
  fi
  local code=$?
  set -e
  echo "$code"
}

sanitize_notes() {
  local text="$1"
  text="${text//"$LOGIN_DIR"/<login-dir>}"
  text="${text//"$WORKDIR"/<workdir>}"
  printf '%s' "$text"
}

prepare_files() {
  local project="$1"
  mkdir -p "$project/sentinel"
  printf 'NOTES_LINE_ALPHA\n' >"$project/NOTES.txt"
  printf 'OTHER_LINE_BETA\n' >"$project/OTHER.txt"
  : >"$project/hooks.jsonl"
}

for entry in "${ROWS_AND_MODES[@]}"; do
  row="${entry%% *}"
  mode="${entry#* }"
  project="$WORKDIR/$row"
  prepare_files "$project"
  log="$project/hooks.jsonl"
  sentinel="$project/sentinel"
  stdout="$project/stdout.txt"
  stderr="$project/stderr.txt"
  SESSION_ID=""
  suppression="off"
  disable=""
  if [[ "$row" == "suppress-settings" ]]; then
    suppression="on"
    disable="disable"
  fi
  if [[ "$row" != "control-user" ]]; then
    mkdir -p "$project/.claude"
    plant_command_hooks "$project/.claude/settings.json" "$log" "$mode" "$sentinel" project
  fi
  plant_command_hooks "$project/user-settings.json" "$log" "$mode" "$sentinel" user "$disable"
  extra=(--settings "$project/user-settings.json")
  if [[ "$row" != "events-permission" ]]; then
    extra+=(--dangerously-skip-permissions)
  fi
  prompt="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=${suppression} NESSA_PHASE=first NESSA_SENTINEL=${sentinel} Reply with OK only."
  case "$row" in
    events-tool|pretool-deny|pretool-ask|pretool-defer|pretool-continue-false|fail-exit-2|fail-exit-1|fail-malformed|fail-timeout)
      prompt="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=${suppression} NESSA_PHASE=first NESSA_SENTINEL=${sentinel} Read NOTES.txt with the Read tool and quote its first line."
      ;;
    pretool-allow-updated)
      prompt="Run exactly this Bash command and do not use any other tool: touch ${sentinel}/hook-sentinel-original"
      ;;
    events-subagent)
      prompt="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=${suppression} NESSA_PHASE=first NESSA_SENTINEL=${sentinel} Use a subagent to read NOTES.txt."
      ;;
    events-permission)
      prompt="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=${suppression} NESSA_PHASE=first NESSA_SENTINEL=${sentinel} Run a shell command that prints PERMISSION_ROW."
      ;;
    prompt-additional-context|prompt-system-message)
      prompt="If any extra context or system message contains a token starting NONCE_ or SYSMSG_, reply with exactly those tokens; otherwise reply NONE."
      ;;
  esac
  code="$(invoke_claude "$project" "$prompt" "$stdout" "$stderr" write "${extra[@]}")"
  notes="CLAUDE_CONFIG_DIR is the login directory <login-dir>. Credentials are not copied. The script does not write that directory, but the Claude CLI writes session transcripts under its projects/ folder. The user hook is --settings inside this project. Each command is tagged --source. claude is started in the project directory. --dangerously-skip-permissions is omitted for events-permission."
  if [[ "$row" == "control-user" ]]; then
    notes="Positive control: user hook only, through --settings, no project settings file. ${notes}"
  fi
  if [[ "$row" == "suppress-settings" ]]; then
    notes="The --settings file contains the user hook and disableAllHooks true. A project hook is also planted. ${notes}"
  fi
  if [[ "$row" == "events-resume" || "$row" == "events-compact" ]]; then
    session="$(thread_id_from "$stdout")"
    if [[ -n "$session" ]]; then
      follow="NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=${suppression} NESSA_PHASE=second NESSA_SENTINEL=${sentinel} Reply with READY only."
      if [[ "$row" == "events-compact" ]]; then
        follow="/compact"
      fi
      follow_code="$(invoke_claude "$project" "$follow" "$stdout" "$stderr" append --resume "$session" "${extra[@]}")"
      if [[ "$follow_code" -ne 0 && "$code" -eq 0 ]]; then
        code="$follow_code"
      fi
      SESSION_ID="$session"
    else
      echo "no session id: ${row} ${suppression}" >&2
      if [[ "$code" -eq 0 ]]; then
        code=1
      fi
    fi
  fi
  if [[ -z "$SESSION_ID" ]]; then
    SESSION_ID="$(thread_id_from "$stdout")"
  fi
  redact_home_fields "$stdout"
  append_row "$row" "$suppression" "$code" "$log" "$stdout" "$stderr" "$sentinel" "$(sanitize_notes "$notes")"
done

if [[ "$MANAGED" -eq 1 ]]; then
  if [[ -f "$MANAGED_PATH" ]]; then
    sudo cp "$MANAGED_PATH" "$MANAGED_BACKUP"
    MANAGED_EXISTED=1
  fi
  sudo mkdir -p /etc/claude-code
  MANAGED_PLANTED=1
  for row in suppress-managed suppress-managed-plus-flag; do
    project="$WORKDIR/$row"
    prepare_files "$project"
    log="$project/hooks.jsonl"
    sentinel="$project/sentinel"
    plant_command_hooks "$project/managed-settings.json" "$log" log "$sentinel" managed
    sudo cp "$project/managed-settings.json" "$MANAGED_PATH"
    SESSION_ID=""
    extra=(--dangerously-skip-permissions)
    notes="Managed hook only, tagged --source managed, at ${MANAGED_PATH}. No project settings and no user --settings. Restored or removed on exit."
    if [[ "$row" == "suppress-managed-plus-flag" ]]; then
      extra+=(--settings '{"disableAllHooks":true}')
      notes="Managed hook tagged --source managed, plus --settings {\"disableAllHooks\":true}. No project settings. Restored or removed on exit."
    fi
    code="$(invoke_claude "$project" \
      "NESSA_HOOK_ROW=${row} NESSA_SUPPRESSION=${row} NESSA_PHASE=first NESSA_SENTINEL=${sentinel} Reply with OK only." \
      "$project/stdout.txt" "$project/stderr.txt" write "${extra[@]}")"
    SESSION_ID="$(thread_id_from "$project/stdout.txt")"
    redact_home_fields "$project/stdout.txt"
    append_row "$row" "$row" "$code" "$log" \
      "$project/stdout.txt" "$project/stderr.txt" "$sentinel" "$(sanitize_notes "$notes")"
  done
fi

finalize_cmd=(
  python3 "$HOOK_BEHAVIOR_ROOT/evidence.py" finalize
  --rows "$ROWS"
  --summary "$SUMMARY"
  --harness "$HARNESS"
  --version "$VERSION"
  --label "$LABEL"
  --claude-reference
)
if [[ -n "$ALLOW_CLAUDE_VERSION" ]]; then
  finalize_cmd+=(--allowed-claude-version "$ALLOW_CLAUDE_VERSION")
fi
set +e
"${finalize_cmd[@]}"
finalize_status=$?
set -e
if [[ "$MANAGED_PLANTED" -eq 1 ]]; then
  restore_status=0
  restore_managed_settings || restore_status=$?
  if [[ "$restore_status" -ne 0 ]]; then
    KEEP_WORKDIR=1
    exit "$restore_status"
  fi
fi
if [[ "$finalize_status" -eq 0 ]]; then
  publish_snapshot "$OUT" "$ROWS" "$SUMMARY"
fi
exit "$finalize_status"
