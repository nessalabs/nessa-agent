"""A stdio MCP server for the MCP client's process tests.

One tool with an MCP App (show_chart), a stateful pair (remember returns a
handle; ui://fixture/handle/<handle> resolves only in the session that gave it
out), where (the process's working directory and environment keys), and exit
(the process exits without answering). With --ignore-eof it keeps running after
its stdin closes, so stopping it has to kill it.
"""
import json
import os
import sys
import time

APP = "text/html;profile=mcp-app"
CHART = "ui://fixture/chart.html"
handles = set()


def answer(message):
    method = message.get("method")
    params = message.get("params") or {}
    if method == "initialize":
        return {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}, "resources": {}},
                "serverInfo": {"name": "fixture-process", "version": "1"}}
    if method == "tools/list":
        return {"tools": [
            {"name": "show_chart", "inputSchema": {"type": "object"}, "_meta": {"ui": {"resourceUri": CHART}}},
            {"name": "remember", "inputSchema": {"type": "object"}},
            {"name": "where", "inputSchema": {"type": "object"}},
            {"name": "exit", "inputSchema": {"type": "object"}},
        ]}
    if method == "tools/call":
        name = params.get("name")
        if name == "remember":
            handle = "h%d" % (len(handles) + 1)
            handles.add(handle)
            return {"content": [{"type": "text", "text": handle}]}
        if name == "where":
            return {"content": [], "structuredContent": {"cwd": os.getcwd(), "env": sorted(os.environ),
                                                          "pid": os.getpid()}}
        if name == "exit":
            sys.stdout.flush()
            os._exit(0)
        return None
    if method == "resources/read":
        uri = params.get("uri", "")
        if uri == CHART:
            return {"contents": [{"uri": uri, "mimeType": APP, "text": "<p>chart</p>"}]}
        handle = uri[len("ui://fixture/handle/"):] if uri.startswith("ui://fixture/handle/") else None
        if handle in handles:
            return {"contents": [{"uri": uri, "mimeType": APP, "text": "<p>%s</p>" % handle}]}
        return None
    if method == "ping":
        return {}
    return None


for line in sys.stdin:
    if not line.strip():
        continue
    message = json.loads(line)
    if "id" not in message or "method" not in message:
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
