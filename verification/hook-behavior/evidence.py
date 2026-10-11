#!/usr/bin/env python3
"""Turn one harness row's files into a JSONL record, then a summary.

The summary JSON is the command's stdout. Progress stays off stdout.
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
    requests = request_text(args.requests, args.row, args.suppression)
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
        "sentinel_files": sentinel_names(args.sentinel),
        "context_nonce_in_model_request": CONTEXT_NONCE in requests,
        "system_nonce_in_model_request": SYSTEM_NONCE in requests,
        "deny_reason_in_stdout": DENY_REASON in excerpt(args.stdout, limit=20000),
        "stdout_excerpt": excerpt(args.stdout),
        "stderr_excerpt": excerpt(args.stderr),
        "notes": args.notes or "",
    }
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
    denied = [row for row in rows if row["row"] == "pretool-deny" and row["suppression"] == "off"]
    deny_events = denied[0]["events"] if denied else []
    deny_ok = (
        bool(denied)
        and "PreToolUse" in deny_events
        and "PostToolUse" not in deny_events
        and denied[0]["deny_reason_in_stdout"]
    )
    checks = {
        "events_present": missing == [],
        "events_missing": missing,
        "disable_all_hooks_empty": suppressed_events == [],
        "pretool_deny_stopped_read_and_reason_quoted": deny_ok,
    }
    checks["ok"] = all(
        [
            checks["events_present"],
            checks["disable_all_hooks_empty"],
            checks["pretool_deny_stopped_read_and_reason_quoted"],
        ]
    )
    return checks


def finalize(args):
    rows = load_rows(args.rows) if Path(args.rows).is_file() else []
    summary = {
        "harness": args.harness,
        "version": args.version,
        "label": args.label,
        "row_count": len(rows),
        "rows": [
            {
                "row": row["row"],
                "suppression": row["suppression"],
                "exit_code": row["exit_code"],
                "events": row["events"],
                "event_counts": row.get("event_counts", {}),
                "tool_names": row.get("tool_names", []),
                "sources": row.get("sources", []),
                "sentinel_files": row["sentinel_files"],
                "context_nonce_in_model_request": row["context_nonce_in_model_request"],
                "system_nonce_in_model_request": row["system_nonce_in_model_request"],
                "deny_reason_in_stdout": row["deny_reason_in_stdout"],
            }
            for row in rows
        ],
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
