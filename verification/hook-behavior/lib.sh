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

# Copy src onto dest only after the copy is complete. A failed copy leaves dest.
publish_evidence() {
  local src="$1" dest="$2" staging="${2}.new"
  if ! cp -f "$src" "$staging"; then
    rm -f "$staging"
    return 1
  fi
  mv -f "$staging" "$dest"
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
