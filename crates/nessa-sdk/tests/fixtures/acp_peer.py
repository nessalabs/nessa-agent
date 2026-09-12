"""Deterministic wire/process fixture; never provider compatibility evidence."""
import json
import os
import pathlib
import signal
import subprocess
import sys
import time

mode = sys.argv[1]
root = pathlib.Path.cwd()
model = os.environ["ANTHROPIC_MODEL"]
assert model == os.environ["ANTHROPIC_CUSTOM_MODEL_OPTION"]
assert os.environ["CLAUDE_CODE_MAX_OUTPUT_TOKENS"] == "100"
session = str(os.getpid())
def record(name, value):
    temporary = root / (name + ".tmp")
    temporary.write_text(value)
    temporary.replace(root / name)

record("pid", session)
pending = None
child = None
permission_id = "provider-permission"

def send(value):
    print(json.dumps({"jsonrpc": "2.0", **value}), flush=True)

def result(id, value):
    send({"id": id, "result": value})

def configs(selected=model, permission_mode="default"):
    return {"configOptions": [{"id": "model", "currentValue": selected}, {"id": "mode", "currentValue": permission_mode}]}

def update(value, sid=session):
    send({"method": "session/update", "params": {"sessionId": sid, "update": value}})

def text(value="hello", sid=session):
    update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": value}}, sid)

def tool(name="Write"):
    return {"toolCallId": "file-1", "title": "Write fixture.txt", "kind": "edit", "status": "pending",
            "_meta": {"claudeCode": {"toolName": name}}, "rawInput": {"file_path": str(root / "fixture.txt"), "content": "fixture"}}

if mode == "startup-stall":
    time.sleep(20)
    sys.exit(0)
if mode == "ignore-stop":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    def reap(_signal, _frame):
        try:
            while os.waitpid(-1, os.WNOHANG)[0]:
                pass
        except ChildProcessError:
            pass
    signal.signal(signal.SIGCHLD, reap)

for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        assert msg["params"]["clientCapabilities"]["terminal"] is False
        result(msg["id"], {"protocolVersion": 1, "agentInfo": {"version": "wrong" if mode == "wrong-version" else "0.76.0"}})
    elif method == "session/new":
        options = msg["params"]["_meta"]["claudeCode"]["options"]
        assert options["model"] == model
        assert options["settingSources"] == []
        assert options["tools"] == ["Read", "Write", "Edit", "Glob", "Grep"]
        assert options["settings"]["disableAllHooks"] is True
        assert options["settings"]["allowedMcpServers"] == []
        assert options["settings"]["permissions"]["ask"] == options["tools"]
        result(msg["id"], {"sessionId": session, **configs("alias" if mode == "wrong-model" else model)})
    elif method == "session/set_config_option":
        assert msg["params"]["value"] == "default"
        result(msg["id"], configs(permission_mode="bypassPermissions" if mode == "wrong-mode" else "default"))
        if mode == "idle-config-change":
            update({"sessionUpdate": "config_option_update", **configs("alias")})
    elif method == "session/prompt":
        assert pending is None
        pending = msg["id"]
        if mode == "malformed":
            print("{broken", flush=True)
        elif mode == "oversize":
            print("x" * 20000, flush=True)
        elif mode == "provider-error":
            send({"id": pending, "error": {"code": -32000, "message": "must not leak provider secret"}})
        elif mode == "wrong-session":
            text(sid="someone-else")
        elif mode == "unknown-reason":
            result(pending, {"stopReason": "new-value"})
        elif mode == "config-change":
            update({"sessionUpdate": "config_option_update", **configs("alias")})
        elif mode == "flood":
            for _ in range(10000):
                text("flood")
        elif mode == "eof":
            sys.exit(0)
        elif mode in ("permission", "permission-stop", "unknown-tool"):
            update({"sessionUpdate": "tool_call", **tool("Bash" if mode == "unknown-tool" else "Write")})
            send({"id": permission_id, "method": "session/request_permission", "params": {
                "sessionId": session, "toolCall": {key: value for key, value in tool().items() if key != "title"}, "options": [
                    {"optionId": "approve-one", "kind": "allow_once", "name": "Allow once"},
                    {"optionId": "deny-one", "kind": "reject_once", "name": "Deny once"},
                    {"optionId": "never-choose", "kind": "allow_always", "name": "Always"}]}})
        elif mode in ("stall", "ignore-stop"):
            if mode == "ignore-stop":
                child = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(20)"])
                record("child-pid", str(child.pid))
            text("running")
        elif mode == "unknown-request":
            send({"id": "unsupported", "method": "terminal/create", "params": {"sessionId": session}})
        else:
            text(msg["params"]["prompt"][0]["text"])
            result(pending, {"stopReason": mode if mode in ("max_tokens", "max_turn_requests", "refusal", "cancelled") else "end_turn"})
            pending = None
    elif method == "session/cancel":
        (root / "cancel-observed").write_text("yes")
        if pending is not None and mode != "ignore-stop":
            result(pending, {"stopReason": "cancelled"})
            pending = None
    elif msg.get("id") == permission_id:
        choice = msg["result"]["outcome"]
        (root / "permission-outcome").write_text(json.dumps(choice))
        if choice["outcome"] == "selected":
            assert choice["optionId"] in ("approve-one", "deny-one")
            if choice["optionId"] == "approve-one":
                (root / "fixture.txt").write_text("fixture")
            update({"sessionUpdate": "tool_call_update", "toolCallId": "file-1", "status": "completed", "content": []})
            result(pending, {"stopReason": "end_turn"})
            pending = None
    elif msg.get("id") == "unsupported":
        assert msg["error"]["code"] == -32601
        result(pending, {"stopReason": "end_turn"})
        pending = None

if mode == "ignore-stop":
    # Keep ownership of/reap the child after TERM; force kills the entire group.
    time.sleep(20)
