"""Temporary #943 probe: publish only a bounded, sanitized fresh failure.

The workflow retains the desktop command's exit status. This script reads only
the output of that new execution, never historical workflow logs.
"""

import collections
import html
import os
import re
import sys

LIMIT = 6000
LINES = 50
ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
PRIMARY = re.compile(r"^(?:error(?:\[[^\]]+\])?:|thread .* panicked at)")
SENSITIVE = re.compile(
    r"(?i)\b([\w-]*(?:token|api[_-]?key|secret|password|credential|authorization|cookie)[\w-]*"
    r"\s*[:=]\s*).*$"
)
URL = re.compile(r"https?://[^\s<>]+")
BEARER = re.compile(r"(?i)\bBearer\s+[^\s'\"<>]+")
# Source/panic payloads can contain private strings or incomplete quotes.
# Keep the compiler's error and source location, not arbitrary quoted values.
QUOTED = re.compile(r'"[^\n]*')


def sanitize(line):
    clean = ANSI.sub("", line)
    clean = URL.sub("<redacted URL>", clean)
    clean = BEARER.sub("Bearer <redacted>", clean)
    clean = SENSITIVE.sub(r"\1<redacted>", clean)
    return QUOTED.sub('"<redacted text>"', clean)


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


def main():
    with open(sys.argv[1], encoding="utf-8", errors="replace") as stream:
        diagnostic = excerpt(stream)
    if not diagnostic:
        diagnostic = "Fresh native desktop command failed without a diagnostic."
    print("::error title=Fresh native desktop failure::" + annotation(diagnostic))
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as output:
            output.write(
                "\n### Fresh native desktop diagnostic (#943)\n\n"
                "From this execution only; the native command's exit status is preserved.\n\n"
                "<pre>" + html.escape(diagnostic) + "</pre>\n"
            )


if __name__ == "__main__":
    main()
