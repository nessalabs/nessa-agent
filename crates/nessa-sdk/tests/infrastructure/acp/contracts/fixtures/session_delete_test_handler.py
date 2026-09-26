"""ACP test handler for session deletion; launched only by the test suite.

Records every method it is sent, in order, so a test can see that nothing but
`initialize`, `session/delete` and `session/list` reached it.

The mode names how it lists its sessions; `+refuse` after it makes it refuse
the delete, as an agent does a session it no longer has.
"""
import json
import os
import pathlib
import sys
import time

mode, _, refuse = sys.argv[1].partition("+")
# Which agent this stands in for, so each binding's `initialize` check passes.
agent_info = {
    "claude": {"version": "0.76.0"},
    "codex": {"name": "@agentclientprotocol/codex-acp", "version": "1.12.0"},
    "opencode": {"name": "OpenCode", "version": "1.18.31"},
}[sys.argv[2] if len(sys.argv) > 2 else "claude"]
asked = sys.argv[3] if len(sys.argv) > 3 else ""
root = pathlib.Path.cwd()
(root / "pid").write_text(str(os.getpid()))
# Where this process was told its home is: a binding that launches in a
# private directory points it there, and must release it.
(root / "home").write_text(os.environ.get("HOME", ""))
methods = []


def send(value):
    print(json.dumps({"jsonrpc": "2.0", **value}), flush=True)


def record(message):
    methods.append({"method": message.get("method"), "params": message.get("params")})
    temporary = root / "methods.tmp"
    temporary.write_text(json.dumps(methods))
    temporary.replace(root / "methods")


for line in sys.stdin:
    message = json.loads(line)
    record(message)
    method = message.get("method")
    if method == "initialize" and mode == "init-refused":
        send({"id": message["id"], "error": {"code": -32002, "message": "not now"}})
    elif method == "initialize":
        capabilities = {"resume": {}} if mode == "not-advertised" else {"resume": {}, "delete": {}}
        if mode.startswith("list-"):
            capabilities["list"] = {}
        send({"id": message["id"], "result": {"protocolVersion": 1, "agentInfo": agent_info, "agentCapabilities": {"sessionCapabilities": capabilities}}})
    elif method == "session/list":
        params = message.get("params") or {}
        other = {"sessionId": "someone-else", "cwd": str(root), "title": "t", "updatedAt": "2026-01-01T00:00:00Z"}
        mine = {"sessionId": asked, "cwd": str(root), "title": "t", "updatedAt": "2026-01-01T00:00:00Z"}
        if mode == "list-error":
            # A code of its own, so a test can tell it from the delete's refusal.
            send({"id": message["id"], "error": {"code": -32000, "message": "store unreadable"}})
        elif mode == "list-listed":
            send({"id": message["id"], "result": {"sessions": [other, mine]}})
        elif mode == "list-paged":
            # Found only on the third page of the workspace's listing.
            page = params.get("cursor")
            if page is None:
                send({"id": message["id"], "result": {"sessions": [other], "nextCursor": "p2"}})
            elif page == "p2":
                send({"id": message["id"], "result": {"sessions": [other], "nextCursor": "p3"}})
            else:
                send({"id": message["id"], "result": {"sessions": [mine]}})
        elif mode == "list-large":
            # One page, no cursor, as Claude sends its whole store: larger than
            # the protocol's usual frame bound, and not naming the session.
            many = [dict(other, sessionId="other-%d" % n) for n in range(20000)]
            send({"id": message["id"], "result": {"sessions": many}})
        elif mode == "list-unreadable":
            # Past even the listing's own bound on bytes.
            many = [dict(other, sessionId="other-%d" % n, title="t" * 200) for n in range(80000)]
            send({"id": message["id"], "result": {"sessions": many}})
        elif mode == "list-too-many-values":
            # Small in bytes, past the listing's bound on values.
            send({"id": message["id"], "result": {"sessions": [{"s": 0}] * 600000}})
        elif mode == "list-no-sessions":
            send({"id": message["id"], "result": {"items": [mine]}})
        elif mode == "list-bad-cursor":
            send({"id": message["id"], "result": {"sessions": [other], "nextCursor": 2}})
        elif mode == "list-stall":
            # Past the list's own startup budget.
            time.sleep(5)
            send({"id": message["id"], "result": {"sessions": [other]}})
        elif mode == "list-malformed":
            # Names the session, but not under `sessionId`.
            send({"id": message["id"], "result": {"sessions": [{"id": asked, "cwd": str(root)}]}})
        elif mode == "list-past-the-bound":
            # Pages past the binding's bound, then one last page that does not
            # name the session: a reader without the bound settles instead of
            # reading forever.
            # The binding's page bound, as the test publishes it.
            page_bound = int(sys.argv[4])
            page = int(params.get("cursor", "0")) + 1
            following = {"nextCursor": str(page)} if page <= page_bound else {}
            send({"id": message["id"], "result": {"sessions": [other], **following}})
        elif mode == "list-cycling-cursor":
            # a -> b -> a: never the same cursor twice in a row.
            following = {None: "a", "a": "b", "b": "a"}[params.get("cursor")]
            send({"id": message["id"], "result": {"sessions": [other], "nextCursor": following}})
        elif mode == "list-stuck-cursor":
            send({"id": message["id"], "result": {"sessions": [other], "nextCursor": "same"}})
        else:
            # `list-unlisted`: the workspace's list does not name it.
            send({"id": message["id"], "result": {"sessions": [other]}})
    elif method == "session/delete":
        if mode == "refused" or refuse:
            send({"id": message["id"], "error": {"code": -32603, "message": "session not found"}})
        elif mode == "stall":
            time.sleep(20)
        elif mode == "bloated":
            # An acceptance holding more values than the protocol allows.
            send({"id": message["id"], "result": {"padding": [0] * 100000}})
        else:
            # A request of the agent's own before it answers: this connection
            # runs nothing it could be about, so it is refused and the
            # delete still completes.
            send({"id": "unrelated", "method": "session/request_permission", "params": {"sessionId": message["params"]["sessionId"]}})
            send({"method": "session/update", "params": {"sessionId": message["params"]["sessionId"], "update": {"sessionUpdate": "available_commands_update", "availableCommands": []}}})
            send({"id": message["id"], "result": {}})
    elif message.get("id") == "unrelated":
        assert message["error"]["code"] == -32601
    else:
        raise AssertionError("unexpected method " + str(method))
