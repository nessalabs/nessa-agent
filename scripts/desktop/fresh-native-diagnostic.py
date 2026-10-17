"""Temporary #943 qualification: bounded, sanitized new execution evidence.

The workflow retains the original command's exit status. This script reads only
the output of that new execution, never historical workflow logs.
"""

import collections
import html
import json
from pathlib import Path
import os
import re
import sys

LIMIT = 6000
LINES = 50
ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
PRIMARY = re.compile(r"^(?:\[[^\]\r\n]{1,200}\] )?(?:error(?:\[[^\]]+\])?:|thread .* panicked at|FAIL\b|ERROR\b|Error:)")
SENSITIVE = re.compile(
    r"(?i)\b([\w-]*(?:token|api[_-]?key|secret|password|credential|authorization|cookie)[\w-]*"
    r"\s*[:=]\s*).*$"
)
URL = re.compile(r"https?://[^\s<>]+", re.IGNORECASE)
BEARER = re.compile(r"(?i)\bBearer\s+[^\s'\"<>]+")
# Source/panic payloads can contain private strings or incomplete quotes.
# Keep the compiler's error and source location, not arbitrary quoted values.
QUOTED = re.compile(r"""['"`][^\n]*""")
PANIC_LOCATION = re.compile(
    r"^(?:\[[A-Za-z0-9_ -]{1,200}\] )?thread [^\r\n]+ panicked at "
    r"([A-Za-z0-9_./\\:-]+\.rs:[0-9]+(?::[0-9]+)*):?\s*$"
)


def sanitize(line):
    clean = ANSI.sub("", line)
    clean = URL.sub("<redacted URL>", clean)
    clean = BEARER.sub("Bearer <redacted>", clean)
    clean = SENSITIVE.sub(r"\1<redacted>", clean)
    # No paired-quote parser: contractions and escaped delimiters cannot make
    # any quoted tail publishable. Preserve only the structural Rust location,
    # never the quoted thread name or panic payload.
    location = PANIC_LOCATION.fullmatch(clean.rstrip("\r\n"))
    if location:
        return "thread <redacted> panicked at " + location[1] + "\n"
    return QUOTED.sub(lambda match: match[0][0] + "<redacted text>" + match[0][0], clean)


def bounded_lines(stream):
    while raw := stream.readline(4096):
        if len(raw) == 4096 and not raw.endswith("\n"):
            # Omit the entire oversized line: a credential delimiter may be
            # beyond the retained prefix. Never attempt redaction on a prefix.
            while rest := stream.readline(4096):
                if rest.endswith("\n"):
                    break
            yield "[oversized diagnostic line omitted]\n"
        else:
            yield raw


def excerpt(stream):
    """Bound both input chunks and retained diagnostic text."""
    selected = []
    tail = collections.deque(maxlen=8)
    size = 0
    for raw in bounded_lines(stream):
        line = sanitize(raw)
        tail.append(line[:512])
        primary = PRIMARY.match(line)
        if primary and selected:
            break
        if primary or selected:
            part = line[: LIMIT - size]
            selected.append(part)
            size += len(part)
            if size >= LIMIT or len(selected) >= LINES:
                break
    return "".join(selected or tail)[:LIMIT].strip()


def annotation(value):
    return value.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def browser_result(directory):
    """Publish only validated provenance and a bounded fresh canonical summary."""
    root = Path(directory)
    record = json.loads((root / "provenance.json").read_text(encoding="utf-8"))
    if record.get("repository") != "nessalabs/nessa-agent":
        raise ValueError("Unexpected provenance repository")
    rows = ["repository: nessalabs/nessa-agent"]
    for key in ["workflowCommit", "source", "base", "tree"]:
        value = record.get(key)
        if key == "base" and value is None:
            rows.append("base: none")
        elif isinstance(value, str) and re.fullmatch(r"[0-9a-f]{40}", value):
            rows.append(f"{key}: {value}")
        else:
            raise ValueError("Invalid provenance commit or tree")
    if record.get("state") != "ready":
        raise ValueError("Browser tree was not ready")
    rows.append("state: ready")
    summary = root / "pr-summary.md"
    if summary.exists():
        with summary.open(encoding="utf-8", errors="replace") as stream:
            lines = []
            size = 0
            for raw in bounded_lines(stream):
                line = sanitize(raw)
                lines.append(line[: LIMIT - size])
                size += len(lines[-1])
                if size >= LIMIT or len(lines) >= LINES:
                    break
        rows.append("".join(lines).strip())
    else:
        rows.append("Canonical summary was not produced; inspect the fresh diagnostic.")
    return "\n".join(rows)


def publish(title, diagnostic, level):
    print(f"::{level} title={title}::" + annotation(diagnostic))
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as output:
            output.write("\n### " + title + "\n\n<pre>" + html.escape(diagnostic) + "</pre>\n")


def main():
    with open(sys.argv[1], encoding="utf-8", errors="replace") as stream:
        diagnostic = excerpt(stream)
    if not diagnostic:
        diagnostic = "Fresh command failed without a diagnostic."
    # Publish a first failure before optional browser provenance so a malformed
    # result document cannot erase the original command diagnostic.
    if os.environ.get("FRESH_COMMAND_STATUS", "1") != "0":
        publish("Fresh qualification failure (#943)", diagnostic, "error")
    if len(sys.argv) == 4 and sys.argv[2] == "--browser-evidence":
        publish("Fresh canonical browser result (#943)", browser_result(sys.argv[3]), "notice")


if __name__ == "__main__":
    main()
