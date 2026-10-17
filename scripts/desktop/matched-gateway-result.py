"""Publish fixed-vocabulary row/request facts from a fresh matched execution."""
import json
import os
from pathlib import Path
import re
import sys

SHA = re.compile(r"[0-9a-f]{40}")
NAMES = {"setup", "handshake", "lists", "opens", "live", "apps", "handshakes", "console", "open", "launch", "gateway-window"}


def checked(value, allowed):
    if value not in allowed:
        raise ValueError("Unexpected diagnostic enum")
    return value


def integer(value):
    if value is None:
        return None
    if type(value) is not int or not 0 <= value <= 10**12:
        raise ValueError("Unexpected diagnostic number")
    return value


def boolean(value):
    if type(value) is not bool:
        raise ValueError("Unexpected diagnostic boolean")
    return value


def result(root, case, candidate, base):
    checked(case, {"candidate", "base"})
    if not SHA.fullmatch(candidate) or not SHA.fullmatch(base):
        raise ValueError("Invalid comparison commit")
    provenance = json.loads((root / "provenance.json").read_text())
    if provenance.get("repository") != "nessalabs/nessa-agent" or provenance.get("state") != "ready":
        raise ValueError("Invalid provenance state")
    safe = {"case": case, "comparisonCandidate": candidate, "comparisonBase": base}
    for name in ["source", "tree", "workflowCommit"]:
        value = provenance.get(name)
        if not isinstance(value, str) or not SHA.fullmatch(value):
            raise ValueError("Invalid provenance identity")
        safe[name] = value
    if safe["source"] != (candidate if case == "candidate" else base):
        raise ValueError("Comparison source does not match provenance")
    document = json.loads((root / "gateway-window/result.json").read_text())
    if document.get("check") != "gateway-window":
        raise ValueError("Wrong result selection")
    safe["selection"] = "canonical dev/claude/bundled; gateway-window scripted; Chromium+WebKit; columns"
    safe["rows"] = []
    rows = document.get("results")
    if not isinstance(rows, list) or not 1 <= len(rows) <= 32:
        raise ValueError("Invalid row count")
    for row in rows:
        failures = row.get("failures")
        if not isinstance(failures, list):
            raise ValueError("Invalid result failures")
        seen = row.get("seen") or {}
        safe["rows"].append({
            "name": checked(row.get("name"), NAMES),
            "engine": checked(row.get("engine"), {None, "chromium", "webkit"}),
            "layout": checked(row.get("layout"), {None, "columns"}),
            "ok": boolean(row.get("ok")),
            "failureCount": len(failures),
            "error": bool(row.get("error")),
            "cannotRun": bool(row.get("cannotRun")),
            "liveMount": seen.get("state") == "live",
        })
    observed = json.loads((root / "matched-requests.json").read_text())
    if observed.get("setup") is not True or not isinstance(observed.get("requests"), list):
        raise ValueError("Request observer did not initialize")
    safe["requests"] = []
    if len(observed["requests"]) > 64:
        raise ValueError("Too many request facts")
    for entry in observed["requests"]:
        item = {
            "event": checked(entry.get("event"), {"failed", "finished", "capture-failed"}),
            "engine": checked(entry.get("engine"), {"chromium", "webkit"}),
        }
        if item["event"] != "capture-failed":
            for key, choices in {
                "endpoint": {"/mcp-resources", "/browser/check", "/desktop.html", "other"},
                "method": {"GET", "POST", "OPTIONS", "other"},
                "resource": {"document", "fetch", "xhr", "script", "stylesheet", "image", "other"},
                "frame": {"main", "child", "unknown"},
            }.items():
                item[key] = checked(entry.get(key), choices)
            for key in ["sameOrigin", "loopback", "duringClose", "pageClosed", "sizeReported", "aborted"]:
                item[key] = boolean(entry.get(key))
            for key in ["status", "contentLength", "bodySize"]:
                item[key] = integer(entry.get(key))
        safe["requests"].append(item)
    safe["observerCoverage"] = {
        engine: any(item.get("engine") == engine and item.get("endpoint") == "/desktop.html" for item in safe["requests"])
        for engine in ["chromium", "webkit"]
    }
    return safe


if __name__ == "__main__":
    value = result(Path(sys.argv[1]), os.environ["MATCHED_CASE"], os.environ["COMPARISON_CANDIDATE"], os.environ["COMPARISON_BASE"])
    # Each record is independently bounded and needs no quoted/log text.
    for kind in ["rows", "requests"]:
        part = {key: val for key, val in value.items() if key not in {"rows", "requests"}}
        part[kind] = value[kind]
        text = json.dumps(part, separators=(",", ":"))
        if len(text) > 24000:
            raise ValueError("Diagnostic annotation exceeds bound")
        print("::notice title=Matched gateway " + kind + " (#943)::" + text.replace("%", "%25").replace("\n", "%0A").replace("\r", "%0D"))
    if not all(value["observerCoverage"].values()):
        sys.exit(2)
