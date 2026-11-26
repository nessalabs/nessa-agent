#!/bin/sh
# A program that asks to come back after the terminal restarts (OSC 7577),
# for the conversation stress check's `resume` step.
#
#   resume-probe.sh kept <word>     says "kept <word>", reports itself, waits
#   resume-probe.sh resumed <word>  what the report asks to be started as:
#                                   says "resumed <word>", reports, waits
#
# The report names this process (`exec` keeps its pid while it waits) and is
# signed with the terminal's token, which the shell's printf builtin never
# puts on a command line. Without a token it reports nothing.
set -eu
word=${2:-probe}
if [ "${1:-}" = resumed ]; then echo "resumed $word"; else echo "kept $word"; fi
self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
if [ -n "${OSC7577_TOKEN:-}" ]; then
  b64() { printf '%s' "$1" | base64 | tr -d '\n'; }
  body="pid=$$:time=$(date +%s):argv=$(b64 "$self"),$(b64 resumed),$(b64 "$word"):cwd=$(b64 "$(pwd -P)")"
  if command -v sha256sum >/dev/null 2>&1; then
    mac=$(printf '%s:%s' "$body" "$OSC7577_TOKEN" | sha256sum | cut -d' ' -f1)
  else
    mac=$(printf '%s:%s' "$body" "$OSC7577_TOKEN" | shasum -a 256 | cut -d' ' -f1)
  fi
  printf '\033]7577;%s:mac=%s\033\\' "$body" "$mac"
fi
unset OSC7577_TOKEN
exec sleep 3600
