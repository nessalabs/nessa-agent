#!/usr/bin/env python3
"""Turn one harness row's files into a JSONL record, then a summary.

The summary JSON is the command's stdout. Progress stays off stdout.

Claude rows store the parsed CLI `result`, `terminal_reason`, and
`permission_denials`. They do not store a tail excerpt. Nonce flags for those
rows come from the result strings and the session transcript under
`CLAUDE_CONFIG_DIR/projects/**/*.jsonl`. Codex and OpenCode keep the scripted
request body and the excerpts.
"""

import argparse
import json
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
    if not directory or not Path(directory).is_dir():
        return ""
    paths = sorted(Path(directory).glob("projects/**/*.jsonl"))
    if session_id:
        matched = []
        for path in paths:
            text = path.read_text(encoding="utf-8", errors="replace")
            if session_id in text or session_id in path.name:
                matched.append(text)
        return "\n".join(matched)
    if not paths:
        return ""
    newest = max(paths, key=lambda path: path.stat().st_mtime)
    return newest.read_text(encoding="utf-8", errors="replace")


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
        seen = "\n".join(results) + "\n" + transcript_text(
            args.transcript_dir, args.session_id
        )
        record["result"] = results
        record["terminal_reason"] = reasons
        record["permission_denials"] = denials
        record["context_nonce_in_model_request"] = CONTEXT_NONCE in seen
        record["system_nonce_in_model_request"] = SYSTEM_NONCE in seen
        record["deny_reason_quoted"] = any(DENY_REASON in text for text in results)
    else:
        requests = request_text(args.requests, args.row, args.suppression)
        stdout = excerpt(args.stdout, limit=20000)
        record["context_nonce_in_model_request"] = CONTEXT_NONCE in requests
        record["system_nonce_in_model_request"] = SYSTEM_NONCE in requests
        record["deny_reason_in_stdout"] = DENY_REASON in stdout
        record["deny_reason_quoted"] = DENY_REASON in stdout
        record["stdout_excerpt"] = excerpt(args.stdout)
        record["stderr_excerpt"] = excerpt(args.stderr)
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
    suppressed = [
        row
        for row in rows
        if row["row"] == "suppress-settings" and row["suppression"] == "on"
    ]
    suppressed_events = suppressed[0]["events"] if suppressed else ["row-missing"]
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
    user_hook_wrote = control is not None and "user" in control.get("hook_sources", [])
    checks = {
        "events_present": missing == [],
        "events_missing": missing,
        "disable_all_hooks_empty": suppressed_events == [],
        "pretool_deny_stopped_read": deny_ok,
        "user_hook_wrote": user_hook_wrote,
        "deny_reason_quoted": bool(denied) and bool(denied.get("deny_reason_quoted")),
    }
    checks["ok"] = all(
        [
            checks["events_present"],
            checks["disable_all_hooks_empty"],
            checks["pretool_deny_stopped_read"],
            checks["user_hook_wrote"],
        ]
    )
    return checks


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
        "context_nonce_in_model_request": row["context_nonce_in_model_request"],
        "system_nonce_in_model_request": row["system_nonce_in_model_request"],
        "deny_reason_quoted": row.get("deny_reason_quoted", False),
    }
    if "result" in row:
        item["result"] = row["result"]
        item["terminal_reason"] = row["terminal_reason"]
        item["permission_denials"] = row["permission_denials"]
    else:
        item["deny_reason_in_stdout"] = row.get("deny_reason_in_stdout", False)
    return item


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
    path = Path(args.summary)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    sys.stdout.write(json.dumps(summary) + "\n")
    if args.claude_reference and not summary["reference_checks"]["ok"]:
        return 1
    return 0


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
