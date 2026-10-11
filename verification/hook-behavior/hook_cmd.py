#!/usr/bin/env python3
"""Command hook planted by the Claude and Codex runners.

stdin is the harness hook JSON. argv is the log path, the output mode, and
the sentinel directory for the PreToolUse rewrite. The log records the event
the harness actually delivered. A mode acts only on the event it belongs to;
every other event is recorded and exits 0.
"""

import json
import sys
import time
from pathlib import Path

DENY_REASON = "NESSA_DENY_REASON"
CONTEXT_NONCE = "NONCE_CONTEXT_4c1e"
SYSTEM_NONCE = "SYSMSG_7ab2"
ORIGINAL = "hook-sentinel-original"
REWRITTEN = "hook-sentinel-rewritten"

PRE_TOOL = "PreToolUse"
PROMPT = "UserPromptSubmit"
STOP = "Stop"


def replace_sentinel(value):
    if isinstance(value, dict):
        return {key: replace_sentinel(item) for key, item in value.items()}
    if isinstance(value, list):
        return [replace_sentinel(item) for item in value]
    if isinstance(value, str):
        return value.replace(ORIGINAL, REWRITTEN)
    return value


def emit(payload):
    sys.stdout.write(json.dumps(payload))
    sys.stdout.write("\n")


def main():
    if len(sys.argv) != 4:
        print("usage: hook_cmd.py LOG MODE SENTINEL", file=sys.stderr)
        return 2
    log_path, mode, sentinel = sys.argv[1:]
    raw = sys.stdin.read()
    try:
        payload = json.loads(raw) if raw.strip() else {}
    except json.JSONDecodeError:
        payload = {"unparsed": raw[:500]}
    event = ""
    tool_name = ""
    source = ""
    if isinstance(payload, dict):
        event = str(payload.get("hook_event_name") or "")
        tool_name = str(payload.get("tool_name") or "")
        source = str(payload.get("source") or payload.get("trigger") or "")
    tool_input = payload.get("tool_input") if isinstance(payload, dict) else None
    output = None
    exit_code = 0

    if mode == "deny" and event == PRE_TOOL:
        output = {
            "hookSpecificOutput": {
                "hookEventName": PRE_TOOL,
                "permissionDecision": "deny",
                "permissionDecisionReason": DENY_REASON,
            }
        }
    elif mode == "allow-updated" and event == PRE_TOOL:
        updated = replace_sentinel(tool_input if isinstance(tool_input, dict) else {})
        if isinstance(updated, dict) and ORIGINAL not in json.dumps(tool_input):
            updated["command"] = f"touch {sentinel}/{REWRITTEN}"
        output = {
            "hookSpecificOutput": {
                "hookEventName": PRE_TOOL,
                "permissionDecision": "allow",
                "updatedInput": updated,
            }
        }
    elif mode == "ask" and event == PRE_TOOL:
        output = {
            "hookSpecificOutput": {
                "hookEventName": PRE_TOOL,
                "permissionDecision": "ask",
                "permissionDecisionReason": "NESSA_ASK",
            }
        }
    elif mode == "defer" and event == PRE_TOOL:
        output = {
            "hookSpecificOutput": {
                "hookEventName": PRE_TOOL,
                "permissionDecision": "defer",
            }
        }
    elif mode == "continue-false" and event == PRE_TOOL:
        output = {"continue": False, "stopReason": "NESSA_STOP_REASON"}
    elif mode == "context" and event == PROMPT:
        output = {
            "hookSpecificOutput": {
                "hookEventName": PROMPT,
                "additionalContext": CONTEXT_NONCE,
            }
        }
    elif mode == "system" and event == PROMPT:
        output = {"systemMessage": SYSTEM_NONCE}
    elif mode == "stop-block" and event == STOP:
        output = {"decision": "block", "reason": "NESSA_STOP_BLOCK"}
    elif mode == "exit2" and event == PRE_TOOL:
        print("NESSA_EXIT_2", file=sys.stderr)
        exit_code = 2
    elif mode == "exit1" and event == PRE_TOOL:
        print("NESSA_EXIT_1", file=sys.stderr)
        exit_code = 1
    elif mode == "malformed" and event == PRE_TOOL:
        sys.stdout.write("{")
        exit_code = 0
        output = "{"

    record = {
        "event": event,
        "tool_name": tool_name,
        "source": source,
        "mode": mode,
        "exit_code": exit_code,
    }
    if output is not None and output != "{":
        record["emitted"] = output
    with Path(log_path).open("a", encoding="utf-8") as log:
        log.write(json.dumps(record) + "\n")
    if isinstance(output, dict):
        emit(output)
    if mode == "timeout" and event == PRE_TOOL:
        time.sleep(30)
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
