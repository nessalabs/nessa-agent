"""ACP test handler for shared transport tests; launched only by the test suite."""
import json
import os
import sys

assert not any(key.startswith(("CLAUDE_", "ANTHROPIC_")) for key in os.environ)
with open("pid", "w") as pid_file:
    pid_file.write(str(os.getpid()))
pending = None
session = "fixture-context"

def send(value):
    print(json.dumps({"jsonrpc": "2.0", **value}), flush=True)

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    params = message.get("params", {})
    if method == "initialize":
        send({"id": message["id"], "result": {"protocolVersion": 1, "agentInfo": {"version": "fixture"}}})
    elif method == "session/new":
        assert set(params) == {"cwd", "mcpServers"}
        send({"id": message["id"], "result": {"sessionId": session}})
    elif method == "session/resume":
        assert params["sessionId"] == session
        send({"id": message["id"], "result": {"sessionId": session}})
    elif method == "session/prompt":
        assert params["sessionId"] == session
        pending = message["id"]
        tool = {"toolCallId": "fixture-read", "title": "Read example", "kind": "read", "rawInput": {"target": "/example.txt"}}
        send({"method": "session/update", "params": {"sessionId": session, "update": {"sessionUpdate": "tool_call", **tool}}})
        send({"id": "review", "method": "session/request_permission", "params": {"sessionId": session, "toolCall": tool, "options": [{"optionId": "allow", "kind": "allow_once", "name": "Read once"}]}})
    elif method == "session/cancel":
        assert params["sessionId"] == session
    elif message.get("id") == "review":
        assert message["result"]["outcome"] == {"outcome": "selected", "optionId": "allow"}
        send({"method": "session/update", "params": {"sessionId": session, "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "fixture complete"}}}})
        send({"id": pending, "result": {"stopReason": "end_turn"}})
    else:
        raise AssertionError("unexpected provider-specific method")
