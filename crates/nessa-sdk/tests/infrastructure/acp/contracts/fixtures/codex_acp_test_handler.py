"""ACP test handler for Codex adapter tests; launched only by the test suite."""
import json
import os
import pathlib
import sys

mode = sys.argv[1]
root = pathlib.Path.cwd()
session = "codex-thread-" + str(os.getpid())

# Codex takes its model and its instructions from its own configuration, not
# from the ACP session request. The launch environment is the whole of what this
# binding gets to say before the protocol starts, so it is checked here.
config = json.loads(os.environ["CODEX_CONFIG"])
model = config["model"]
assert os.environ["INITIAL_AGENT_MODE"] == "read-only"
assert os.environ["NO_BROWSER"] == "1"
if mode == "instructions":
    assert config["instructions"] == "Core instructions.\nPlugin instructions."
else:
    assert "instructions" not in config

(root / "pid").write_text(str(os.getpid()))
selected = "codex-default"
configured_steps = []
pending = None


def send(value):
    print(json.dumps({"jsonrpc": "2.0", **value}), flush=True)


def result(id, value):
    send({"id": id, "result": value})


def update(value):
    send({"method": "session/update", "params": {"sessionId": session, "update": value}})


def text(value):
    update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": value}})


def configs():
    offered = ["codex-default"] if mode == "model-not-offered" else ["codex-default", model]
    current = "agent-full-access" if mode == "mode-refused" else "read-only"
    return {"configOptions": [
        {"id": "model", "currentValue": selected,
         "options": [{"value": value} for value in offered]},
        {"id": "reasoning_effort", "currentValue": "medium"},
        {"id": "mode", "currentValue": current},
    ]}


def command_tool_call():
    return {
        "sessionUpdate": "tool_call",
        "toolCallId": "command-1",
        "name": "exec_command",
        "kind": "execute",
        "title": "npm test",
        "status": "pending",
        "rawInput": {"command": "npm test", "cwd": str(root)},
        "content": [{"type": "terminal", "terminalId": "command-1"}],
        "_meta": {"terminal_info": {"cwd": str(root), "terminal_id": "command-1"}},
    }


for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        name = "@example/codex-acp" if mode == "wrong-harness" else "@agentclientprotocol/codex-acp"
        version = "9.9.9" if mode == "wrong-version" else "1.12.0"
        result(msg["id"], {
            "protocolVersion": 1,
            "agentInfo": {"name": name, "title": "Codex", "version": version},
            "agentCapabilities": {"sessionCapabilities": {"resume": {}}},
            "_meta": {"steering": {"supported": True}},
        })
    elif method in ("session/new", "session/resume"):
        params = msg["params"]
        # Codex is told the workspace and the trusted MCP servers, and nothing
        # else: a model or a tool policy arriving here would be this binding
        # configuring a session Codex does not configure that way.
        assert set(params) <= {"cwd", "mcpServers", "sessionId"}
        assert params["cwd"] == str(root)
        assert params["mcpServers"] == []
        if method == "session/resume":
            session = params["sessionId"]
        result(msg["id"], {"sessionId": session, **configs()})
    elif method == "session/set_config_option":
        params = msg["params"]
        assert params["sessionId"] == session
        configured_steps.append(params["configId"])
        if params["configId"] == "model":
            assert params["value"] == model
            # Codex reporting its own configuration while this binding is still
            # applying it: the model it names is the one it had, because the
            # selection being answered here has not been made yet.
            if mode == "startup-update-configuring":
                update({"sessionUpdate": "config_option_update", **configs()})
            if mode == "model-refused":
                send({"id": msg["id"], "error": {"code": -32042, "message": "unknown model"}})
                continue
            selected = model
        else:
            # The model is always settled first: a refused model must not leave
            # a session that has been put into its approval mode anyway.
            assert configured_steps == ["model", "mode"], configured_steps
            assert params["value"] == "read-only"
        result(msg["id"], configs())
    elif method == "session/prompt":
        pending = msg["id"]
        if mode == "terminal-command":
            update(command_tool_call())
            update({"sessionUpdate": "tool_call_update", "toolCallId": "command-1",
                    "_meta": {"terminal_output_delta": {"data": "2 passed\n", "terminal_id": "command-1"}}})
            update({"sessionUpdate": "tool_call_update", "toolCallId": "command-1", "status": "completed",
                    "rawOutput": {"formatted_output": "2 passed\n", "exit_code": 0},
                    "_meta": {"terminal_exit": {"exit_code": 0, "signal": None, "terminal_id": "command-1"}}})
        elif mode == "file-change-permission":
            # A file change is asked for with an identifier, a kind and a status,
            # and what it is asking about is in the request's own metadata.
            send({"id": "file-review", "method": "session/request_permission", "params": {
                "sessionId": session,
                "toolCall": {"toolCallId": "file-change-1", "kind": "edit", "status": "pending"},
                "options": [
                    {"optionId": "allow_once", "kind": "allow_once", "name": "Allow Once"},
                    {"optionId": "allow_always", "kind": "allow_always", "name": "Allow for Session"},
                    {"optionId": "reject_once", "kind": "reject_once", "name": "Reject"},
                ],
                "_meta": {"codex": {"params": {"itemId": "file-change-1", "reason": "Modifying config file"}}},
            }})
            continue
        else:
            text(msg["params"]["prompt"][0]["text"])
        result(pending, {"stopReason": "end_turn"})
        pending = None
    elif method == "session/cancel":
        if pending is not None:
            result(pending, {"stopReason": "cancelled"})
            pending = None
    elif msg.get("id") == "file-review":
        outcome = msg["result"]["outcome"]
        (root / "permission-outcome").write_text(json.dumps(outcome))
        update({"sessionUpdate": "tool_call_update", "toolCallId": "file-change-1", "status": "completed"})
        result(pending, {"stopReason": "end_turn"})
        pending = None
