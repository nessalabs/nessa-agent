#!/usr/bin/env python3
"""Turn one harness row's files into a JSONL record, then a summary.

The summary JSON is the command's stdout. Progress stays off stdout.

Claude prompt rows store only the nonce booleans. Every other Claude row
stores `result` capped at 200 characters, plus `terminal_reason` and
`permission_denials`. `*_nonce_in_result` is what the model echoed.
`*_nonce_in_transcript` is presence in
`CLAUDE_CONFIG_DIR/projects/<project>/<session-id>.jsonl`. An empty session id
leaves those booleans false. Transcript presence is not a model request.
Codex and OpenCode keep `*_nonce_in_model_request` from the scripted
request body, and they keep the excerpts. `finalize` refuses stored text that
contains an email address or the home directory. After it writes the summary,
it exits 1 when a row's exit code is not the expected one: 0, except Codex
`stop-decision-block` with suppression off, which is 124.
"""

import argparse
import json
import os
import re
import sys
from collections import Counter
from pathlib import Path

CONTEXT_NONCE = "NONCE_CONTEXT_4c1e"
SYSTEM_NONCE = "SYSMSG_7ab2"
DENY_REASON = "NESSA_DENY_REASON"
REFERENCE_EVENTS = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "SessionEnd",
]
PROMPT_ROWS = {"prompt-additional-context", "prompt-system-message"}
RESULT_LIMIT = 200
EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")


def excerpt(path, limit=1500):
    if not path or not Path(path).is_file():
        return ""
    text = Path(path).read_text(encoding="utf-8", errors="replace")
    if len(text) <= limit:
        return text
    return text[-limit:]


def records_from_log(path):
    found = []
    if not path or not Path(path).is_file():
        return found
    for line in Path(path).read_text(encoding="utf-8", errors="replace").splitlines():
        if not line.strip():
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            found.append({"event": "unparsed"})
            continue
        if isinstance(record, dict):
            found.append(record)
    return found


def request_text(directory, row, suppression):
    if not directory or not Path(directory).is_dir():
        return ""
    needle_row = f"NESSA_HOOK_ROW={row}"
    needle_suppression = f"NESSA_SUPPRESSION={suppression}"
    chunks = []
    for path in sorted(Path(directory).glob("req-*.json")):
        text = path.read_text(encoding="utf-8", errors="replace")
        if needle_row in text and needle_suppression in text:
            chunks.append(text)
    return "\n".join(chunks)


def json_values(path):
    if not path or not Path(path).is_file():
        return []
    text = Path(path).read_text(encoding="utf-8", errors="replace")
    decoder = json.JSONDecoder()
    found = []
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
        found.append(value)
    return found


def claude_results(path):
    found = []
    for value in json_values(path):
        if isinstance(value, dict) and (
            value.get("type") == "result" or "permission_denials" in value
        ):
            found.append(value)
    return found


def transcript_text(directory, session_id):
    if not directory or not session_id or not Path(directory).is_dir():
        return ""
    paths = sorted(Path(directory).glob(f"projects/*/{session_id}.jsonl"))
    return "\n".join(
        path.read_text(encoding="utf-8", errors="replace") for path in paths
    )


def sentinel_names(directory):
    if not directory or not Path(directory).is_dir():
        return []
    return sorted(path.name for path in Path(directory).iterdir() if path.is_file())


def append(args):
    logged = records_from_log(args.log)
    events = [str(item.get("event") or "") for item in logged if item.get("event")]
    tool_names = []
    for item in logged:
        if item.get("tool_name"):
            tool_names.append(str(item["tool_name"]))
            continue
        detail = item.get("detail")
        if isinstance(detail, dict) and detail.get("tool"):
            tool_names.append(str(detail["tool"]))
    sources = [
        f"{item.get('event')}:{item.get('source')}"
        for item in logged
        if item.get("event") and item.get("source")
    ]
    hook_sources = [
        str(item["hook_source"]) for item in logged if item.get("hook_source")
    ]
    record = {
        "harness": args.harness,
        "version": args.version,
        "row": args.row,
        "suppression": args.suppression,
        "label": args.label,
        "exit_code": args.exit_code,
        "events": list(dict.fromkeys(events)),
        "event_counts": dict(Counter(events)),
        "tool_names": list(dict.fromkeys(tool_names)),
        "sources": list(dict.fromkeys(sources)),
        "hook_sources": list(dict.fromkeys(hook_sources)),
        "sentinel_files": sentinel_names(args.sentinel),
        "notes": args.notes or "",
    }
    if args.harness == "claude":
        messages = claude_results(args.stdout)
        results = [
            message["result"]
            for message in messages
            if isinstance(message.get("result"), str)
        ]
        reasons = [
            message["terminal_reason"]
            for message in messages
            if message.get("terminal_reason") is not None
        ]
        denials = []
        for message in messages:
            value = message.get("permission_denials")
            if isinstance(value, list):
                denials.extend(value)
        transcript = transcript_text(args.transcript_dir, args.session_id)
        record["context_nonce_in_result"] = any(CONTEXT_NONCE in text for text in results)
        record["system_nonce_in_result"] = any(SYSTEM_NONCE in text for text in results)
        record["context_nonce_in_transcript"] = CONTEXT_NONCE in transcript
        record["system_nonce_in_transcript"] = SYSTEM_NONCE in transcript
        record["deny_reason_quoted"] = any(DENY_REASON in text for text in results)
        if args.row not in PROMPT_ROWS:
            record["result"] = [text[:RESULT_LIMIT] for text in results]
            record["terminal_reason"] = reasons
            record["permission_denials"] = denials
    else:
        requests = request_text(args.requests, args.row, args.suppression)
        stdout = excerpt(args.stdout, limit=20000)
        record["context_nonce_in_model_request"] = CONTEXT_NONCE in requests
        record["system_nonce_in_model_request"] = SYSTEM_NONCE in requests
        record["deny_reason_in_stdout"] = DENY_REASON in stdout
        record["deny_reason_quoted"] = DENY_REASON in stdout
        record["stdout_excerpt"] = excerpt(args.stdout)
        record["stderr_excerpt"] = excerpt(args.stderr)
    if privacy_violation(record):
        print(
            "stored row contains an email address or the home directory",
            file=sys.stderr,
        )
        raise SystemExit(1)
    path = Path(args.rows)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record) + "\n")


def load_rows(path):
    rows = []
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        if line.strip():
            rows.append(json.loads(line))
    return rows


def claude_reference(rows):
    fired = []
    for row in rows:
        if row["row"] in ("events-plain", "events-tool") and row["suppression"] == "off":
            fired.extend(row["events"])
    present = set(fired)
    missing = [name for name in REFERENCE_EVENTS if name not in present]
    plain = next(
        (
            row
            for row in rows
            if row["row"] == "events-plain" and row["suppression"] == "off"
        ),
        None,
    )
    plain_sources = plain.get("hook_sources") if plain else None
    if not isinstance(plain_sources, list):
        plain_sources = []
    if "project" not in plain_sources:
        missing.append("hook_source:project")
    if "user" not in plain_sources:
        missing.append("hook_source:user")
    plain_exit_ok = plain is not None and plain.get("exit_code") == 0
    suppressed = next(
        (
            row
            for row in rows
            if row["row"] == "suppress-settings" and row["suppression"] == "on"
        ),
        None,
    )
    reasons = suppressed.get("terminal_reason") if suppressed else None
    if not isinstance(reasons, list):
        reasons = []
    hooks_empty = (
        suppressed is not None
        and suppressed.get("exit_code") == 0
        and "completed" in reasons
        and suppressed.get("events") == []
    )
    denied = next(
        (
            row
            for row in rows
            if row["row"] == "pretool-deny" and row["suppression"] == "off"
        ),
        None,
    )
    denials = denied.get("permission_denials") if denied else None
    deny_ok = (
        denied is not None
        and isinstance(denials, list)
        and len(denials) >= 1
        and "PostToolUse" not in denied.get("events", [])
    )
    control = next((row for row in rows if row["row"] == "control-user"), None)
    control_sources = control.get("hook_sources") if control else None
    if not isinstance(control_sources, list):
        control_sources = []
    user_hook_wrote = (
        control is not None
        and control.get("exit_code") == 0
        and "user" in control_sources
    )
    rewritten = next(
        (
            row
            for row in rows
            if row["row"] == "pretool-allow-updated" and row["suppression"] == "off"
        ),
        None,
    )
    allow_ok = (
        rewritten is not None
        and rewritten.get("sentinel_files") == ["hook-sentinel-rewritten"]
    )
    checks = {
        "events_present": missing == [] and plain_exit_ok,
        "events_missing": missing,
        "disable_all_hooks_empty": hooks_empty,
        "pretool_deny_stopped_read": deny_ok,
        "pretool_allow_updated_rewritten_only": allow_ok,
        "user_hook_wrote": user_hook_wrote,
        "deny_reason_quoted": bool(denied) and bool(denied.get("deny_reason_quoted")),
    }
    checks["ok"] = all(
        [
            checks["events_present"],
            checks["disable_all_hooks_empty"],
            checks["pretool_deny_stopped_read"],
            checks["pretool_allow_updated_rewritten_only"],
            checks["user_hook_wrote"],
        ]
    )
    return checks


def stored_strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for item in value.values():
            yield from stored_strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from stored_strings(item)


def privacy_violation(value):
    home = os.environ.get("HOME") or ""
    home_path = home if len(home) > 1 else ""
    for text in stored_strings(value):
        if EMAIL.search(text):
            return "email"
        if home_path and home_path in text:
            return "home"
    return None


def summary_row(row):
    item = {
        "row": row["row"],
        "suppression": row["suppression"],
        "exit_code": row["exit_code"],
        "events": row["events"],
        "event_counts": row.get("event_counts", {}),
        "tool_names": row.get("tool_names", []),
        "sources": row.get("sources", []),
        "hook_sources": row.get("hook_sources", []),
        "sentinel_files": row["sentinel_files"],
        "deny_reason_quoted": row.get("deny_reason_quoted", False),
    }
    if "context_nonce_in_result" in row:
        item["context_nonce_in_result"] = row["context_nonce_in_result"]
        item["system_nonce_in_result"] = row["system_nonce_in_result"]
        item["context_nonce_in_transcript"] = row["context_nonce_in_transcript"]
        item["system_nonce_in_transcript"] = row["system_nonce_in_transcript"]
    else:
        item["context_nonce_in_model_request"] = row["context_nonce_in_model_request"]
        item["system_nonce_in_model_request"] = row["system_nonce_in_model_request"]
    if "result" in row:
        item["result"] = row["result"]
        item["terminal_reason"] = row["terminal_reason"]
        item["permission_denials"] = row["permission_denials"]
    elif "deny_reason_in_stdout" in row:
        item["deny_reason_in_stdout"] = row["deny_reason_in_stdout"]
    return item


def expected_exit(harness, row):
    if (
        harness == "codex"
        and row["row"] == "stop-decision-block"
        and row["suppression"] == "off"
    ):
        return 124
    return 0


def unexpected_exit_lines(harness, rows):
    lines = []
    for row in rows:
        expected = expected_exit(harness, row)
        actual = row.get("exit_code")
        if actual != expected:
            lines.append(
                f"unexpected exit: {row['row']} {row['suppression']} "
                f"exit_code={actual} expected {expected}"
            )
    return lines


def finalize(args):
    rows = load_rows(args.rows) if Path(args.rows).is_file() else []
    summary = {
        "harness": args.harness,
        "version": args.version,
        "label": args.label,
        "row_count": len(rows),
        "rows": [summary_row(row) for row in rows],
    }
    if args.claude_reference:
        summary["reference_checks"] = claude_reference(rows)
    if privacy_violation(rows) or privacy_violation(summary):
        print(
            "stored text contains an email address or the home directory",
            file=sys.stderr,
        )
        return 1
    path = Path(args.summary)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    sys.stdout.write(json.dumps(summary) + "\n")
    status = 0
    if args.claude_reference and not summary["reference_checks"]["ok"]:
        status = 1
    for line in unexpected_exit_lines(args.harness, rows):
        print(line, file=sys.stderr)
        status = 1
    return status


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    add = sub.add_parser("append")
    add.add_argument("--rows", required=True)
    add.add_argument("--harness", required=True)
    add.add_argument("--version", required=True)
    add.add_argument("--row", required=True)
    add.add_argument("--suppression", required=True)
    add.add_argument("--label", required=True)
    add.add_argument("--exit-code", required=True, type=int)
    add.add_argument("--log", required=True)
    add.add_argument("--stdout", required=True)
    add.add_argument("--stderr", required=True)
    add.add_argument("--sentinel", required=True)
    add.add_argument("--requests", default="")
    add.add_argument("--transcript-dir", default="")
    add.add_argument("--session-id", default="")
    add.add_argument("--notes", default="")
    done = sub.add_parser("finalize")
    done.add_argument("--rows", required=True)
    done.add_argument("--summary", required=True)
    done.add_argument("--harness", required=True)
    done.add_argument("--version", required=True)
    done.add_argument("--label", required=True)
    done.add_argument("--claude-reference", action="store_true")
    args = parser.parse_args()
    if args.command == "append":
        append(args)
        return 0
    return finalize(args)


if __name__ == "__main__":
    sys.exit(main())
