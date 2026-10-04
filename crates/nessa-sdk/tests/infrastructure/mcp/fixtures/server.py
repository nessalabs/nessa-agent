"""A stdio MCP server for the MCP client's process tests.

One tool with an MCP App (show_chart), a stateful pair (remember returns a
handle; ui://fixture/handle/<handle> resolves only in the session that gave it
out), where (the process's working directory and environment keys), and exit
(the process exits without answering), and sleep (answers after 30 s). With
--ignore-eof it keeps running after its stdin closes, so stopping it has to
kill it; with --child it starts a child process of its own (`where` names it),
which stopping it has to stop too.

For a host's inspection of a server: --silent never answers anything;
--silent-on-list answers `initialize` and never answers `tools/list`,
writing PATH once asked when --listed-file PATH is given;
--exit-on-list exits when its tools are listed; --pages N lists one tool
per page over N pages; --apps N lists N tools, each with an MCP App of its
own (ui://fixture/app-<i>.html, asking for a CSP and the camera);
--pid-file PATH writes the process id to PATH once started; and
--child-pid-file PATH writes the --child's process id to PATH.
"""
import json
import os
import subprocess
import sys
import time

APP = "text/html;profile=mcp-app"
CHART = "ui://fixture/chart.html"
handles = set()
child = subprocess.Popen(["/bin/sleep", "300"]) if "--child" in sys.argv else None


def option(name):
    """The value given after `name`, or None."""
    return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else None


if option("--pid-file"):
    with open(option("--pid-file"), "w") as pid_file:
        pid_file.write(str(os.getpid()))
if child and option("--child-pid-file"):
    with open(option("--child-pid-file"), "w") as child_pid_file:
        child_pid_file.write(str(child.pid))
PAGES = int(option("--pages") or 0)
APPS = int(option("--apps") or 0)


def answer(message):
    method = message.get("method")
    params = message.get("params") or {}
    if method == "initialize":
        return {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}, "resources": {}},
                "serverInfo": {"name": "fixture-process", "version": "1"}}
    if method == "tools/list" and "--exit-on-list" in sys.argv:
        sys.stdout.flush()
        os._exit(0)
    if method == "tools/list" and PAGES:
        page = int((params.get("cursor") or "0"))
        result = {"tools": [{"name": "page_%d" % page, "inputSchema": {"type": "object"},
                             "annotations": {"readOnlyHint": True}}]}
        if page + 1 < PAGES:
            result["nextCursor"] = str(page + 1)
        return result
    if method == "tools/list" and APPS:
        return {"tools": [{"name": "app_%d" % i, "inputSchema": {"type": "object"},
                           "annotations": {"destructiveHint": False},
                           "_meta": {"ui": {"resourceUri": "ui://fixture/app-%d.html" % i}}}
                          for i in range(APPS)]}
    if method == "tools/list":
        return {"tools": [
            {"name": "show_chart", "inputSchema": {"type": "object"}, "_meta": {"ui": {"resourceUri": CHART}}},
            {"name": "remember", "inputSchema": {"type": "object"}},
            {"name": "where", "inputSchema": {"type": "object"}},
            {"name": "exit", "inputSchema": {"type": "object"}},
            {"name": "sleep", "inputSchema": {"type": "object"}},
        ]}
    if method == "tools/call":
        name = params.get("name")
        if name == "remember":
            handle = "h%d" % (len(handles) + 1)
            handles.add(handle)
            return {"content": [{"type": "text", "text": handle}]}
        if name == "where":
            return {"content": [], "structuredContent": {"cwd": os.getcwd(), "env": sorted(os.environ),
                                                          "pid": os.getpid(),
                                                          "child": child.pid if child else None}}
        if name == "sleep":
            time.sleep(30)
            return {"content": [{"type": "text", "text": "slept"}]}
        if name == "exit":
            sys.stdout.flush()
            os._exit(0)
        return None
    if method == "resources/read":
        uri = params.get("uri", "")
        if uri == CHART:
            return {"contents": [{"uri": uri, "mimeType": APP, "text": "<p>chart</p>"}]}
        if uri.startswith("ui://fixture/app-"):
            return {"contents": [{"uri": uri, "mimeType": APP, "text": "<p>app</p>",
                                  "_meta": {"ui": {"csp": {"connectDomains": ["https://api.example.com"]},
                                                   "permissions": {"camera": {}}}}}]}
        handle = uri[len("ui://fixture/handle/"):] if uri.startswith("ui://fixture/handle/") else None
        if handle in handles:
            return {"contents": [{"uri": uri, "mimeType": APP, "text": "<p>%s</p>" % handle}]}
        return None
    if method == "ping":
        return {}
    return None


for line in sys.stdin:
    if not line.strip() or "--silent" in sys.argv:
        continue
    message = json.loads(line)
    if "id" not in message or "method" not in message:
        continue
    if message["method"] == "tools/list" and "--silent-on-list" in sys.argv:
        if option("--listed-file"):
            with open(option("--listed-file"), "w") as listed_file:
                listed_file.write("asked")
        continue
    result = answer(message)
    if result is None:
        reply = {"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32002, "message": "not found"}}
    else:
        reply = {"jsonrpc": "2.0", "id": message["id"], "result": result}
    sys.stdout.write(json.dumps(reply) + "\n")
    sys.stdout.flush()

if "--ignore-eof" in sys.argv:
    while True:
        time.sleep(60)
