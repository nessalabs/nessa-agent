# Shared helpers for the hook-behavior runners.
# A runner sources this file. Diagnostics go to stderr.

set -euo pipefail

hook_behavior_root() {
  cd "$(dirname "${BASH_SOURCE[1]}")"
  pwd
}

plant_command_hooks() {
  local dest="$1" log="$2" mode="$3" sentinel="$4" source="$5" disable="${6:-}"
  python3 - "$dest" "$log" "$mode" "$sentinel" "$(command -v python3)" "$HOOK_BEHAVIOR_ROOT/hook_cmd.py" "$source" "$disable" <<'PY'
import json
import shlex
import sys

dest, log, mode, sentinel, python, hook, source, disable = sys.argv[1:9]
events = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
    "Stop",
    "SessionEnd",
]
timeout = 1 if mode == "timeout" else 30
command = " ".join(
    shlex.quote(part)
    for part in (python, hook, "--source", source, log, mode, sentinel)
)
hooks = {}
for event in events:
    handler = {"type": "command", "command": command}
    if event == "PreToolUse":
        handler["timeout"] = timeout
    hooks[event] = [{"hooks": [handler]}]
document = {"hooks": hooks}
if disable == "disable":
    document["disableAllHooks"] = True
with open(dest, "w", encoding="utf-8") as handle:
    json.dump(document, handle)
PY
}

thread_id_from() {
  python3 - "$1" <<'PY'
import json
import sys

found = ""

def walk(value):
    global found
    if found:
        return
    if isinstance(value, dict):
        for key, item in value.items():
            if key in ("thread_id", "threadId", "session_id", "sessionId", "sessionID") and isinstance(item, str) and len(item) > 8:
                found = item
                return
            walk(item)
    elif isinstance(value, list):
        for item in value:
            walk(item)

text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
decoder = json.JSONDecoder()
index = 0
length = len(text)
while index < length and not found:
    while index < length and text[index].isspace():
        index += 1
    if index >= length:
        break
    try:
        value, end = decoder.raw_decode(text, index)
    except json.JSONDecodeError:
        break
    walk(value)
    index = end
print(found)
PY
}

# Replace dest with a directory that already holds one complete snapshot.
# renameat2 exchanges the two directories in one call. Without that call, move
# dest aside and move the snapshot into its place, and put dest back if the
# second move fails.
exchange_snapshot() {
  python3 - "$1" "$2" <<'PY'
import ctypes
import os
import sys

src, dst = sys.argv[1:]
AT_FDCWD = -100
RENAME_EXCHANGE = 2
libc = ctypes.CDLL(None, use_errno=True)
try:
    renameat2 = libc.renameat2
except AttributeError:
    raise SystemExit(2)
renameat2.argtypes = [
    ctypes.c_int,
    ctypes.c_char_p,
    ctypes.c_int,
    ctypes.c_char_p,
    ctypes.c_uint,
]
renameat2.restype = ctypes.c_int
result = renameat2(
    AT_FDCWD,
    os.fsencode(src),
    AT_FDCWD,
    os.fsencode(dst),
    RENAME_EXCHANGE,
)
if result != 0:
    print(os.strerror(ctypes.get_errno()), file=sys.stderr)
    raise SystemExit(1)
PY
}

rename_snapshot() {
  local stage="$1" dest="$2" parent previous
  parent="$(dirname "$dest")"
  previous="$(mktemp -d "$parent/.snapshot-old.XXXXXX")" || return 1
  rmdir "$previous" || {
    rm -rf "$previous"
    return 1
  }
  if ! mv "$dest" "$previous"; then
    return 1
  fi
  if ! mv "$stage" "$dest"; then
    mv "$previous" "$dest" || echo "error: could not restore $dest from $previous" >&2
    return 1
  fi
  rm -rf "$previous"
}

release_snapshot_lock() {
  if [[ -n "${lock_fd:-}" ]]; then
    flock -u "$lock_fd" || true
    exec {lock_fd}<&-
  fi
}

# Publish rows.jsonl and summary.json as one snapshot. A failed copy leaves
# dest unchanged. On Linux the swap is one rename, so the two files stay from
# one run. Without that rename, the directory is moved aside and put back if
# the replacement fails.
publish_snapshot() {
  local out="${1%/}" rows="$2" summary="$3"
  local parent base stage lock_fd name bn exchange_status=0
  mkdir -p "$(dirname "$out")"
  parent="$(cd "$(dirname "$out")" && pwd)"
  base="$(basename "$out")"
  out="$parent/$base"
  exec {lock_fd}<"$parent" || return 1
  flock "$lock_fd" || {
    release_snapshot_lock
    return 1
  }
  mkdir -p "$out"
  stage="$(mktemp -d "$parent/.$base.XXXXXX")" || {
    release_snapshot_lock
    return 1
  }
  while IFS= read -r -d '' name; do
    bn="$(basename "$name")"
    if [[ "$bn" == "rows.jsonl" || "$bn" == "summary.json" ]]; then
      continue
    fi
    if ! cp -a "$name" "$stage/$bn"; then
      rm -rf "$stage"
      release_snapshot_lock
      return 1
    fi
  done < <(find "$out" -mindepth 1 -maxdepth 1 -print0)
  if ! cp -f "$rows" "$stage/rows.jsonl" || ! cp -f "$summary" "$stage/summary.json"; then
    rm -rf "$stage"
    release_snapshot_lock
    return 1
  fi
  exchange_snapshot "$stage" "$out" || exchange_status=$?
  if [[ "$exchange_status" -eq 2 ]]; then
    if ! rename_snapshot "$stage" "$out"; then
      if [[ -d "$stage" ]]; then
        rm -rf "$stage"
      fi
      release_snapshot_lock
      return 1
    fi
  elif [[ "$exchange_status" -ne 0 ]]; then
    rm -rf "$stage"
    release_snapshot_lock
    return 1
  else
    case "$stage" in
      "$parent/.$base."*) rm -rf "$stage" ;;
      *)
        echo "error: refusing to remove $stage" >&2
        release_snapshot_lock
        return 1
        ;;
    esac
  fi
  release_snapshot_lock
}

append_row() {
  python3 "$HOOK_BEHAVIOR_ROOT/evidence.py" append \
    --rows "$ROWS" \
    --harness "$HARNESS" \
    --version "$VERSION" \
    --row "$1" \
    --suppression "$2" \
    --label "$LABEL" \
    --exit-code "$3" \
    --log "$4" \
    --stdout "$5" \
    --stderr "$6" \
    --sentinel "$7" \
    --requests "${REQUESTS:-}" \
    --transcript-dir "${TRANSCRIPT_DIR:-}" \
    --session-id "${SESSION_ID:-}" \
    --notes "${8:-}"
}

start_scripted_server() {
  REQUESTS="$WORKDIR/requests"
  mkdir -p "$REQUESTS"
  python3 "$HOOK_BEHAVIOR_ROOT/scripted_openai.py" "$REQUESTS" >"$WORKDIR/port" 2>"$WORKDIR/server.err" &
  SERVER_PID=$!
  local attempt
  for attempt in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if [[ -s "$WORKDIR/port" ]]; then
      PORT="$(cat "$WORKDIR/port")"
      return 0
    fi
    sleep 0.1
  done
  echo "scripted server did not print a port" >&2
  return 1
}

stop_scripted_server() {
  if [[ -n "${SERVER_PID:-}" ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
}
