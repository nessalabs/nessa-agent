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
        # Where the only live run against a real Codex ended. Its adapter
        # refuses here, before any session exists, when nothing has signed it
        # in — the client sent no `authenticate` and the launch named no
        # default sign-in request. Covered so the binding's answer on that path
        # is a contract rather than something only a live run ever sees.
        if mode == "not-signed-in":
            send({"id": msg["id"], "error": {"code": -32000, "message": "Authentication required"}})
            continue
        params = msg["params"]
        # Codex is told the workspace and the trusted MCP servers, and nothing
        # else: a model or a tool policy arriving here would be this binding
        # configuring a session Codex does not configure that way.
        assert set(params) <= {"cwd", "mcpServers", "sessionId"}
        # Compared as resolved paths. `Path.cwd()` is the physical directory,
        # and on macOS a temporary directory reaches this process as a symlink
        # into /private, so comparing the strings would fail everywhere the
        # workspace is correct but spelled the other way.
        assert pathlib.Path(params["cwd"]).resolve() == root
        assert params["mcpServers"] == []
        if method == "session/resume":
            session = params["sessionId"]
            # Named so a test can tell a resumed session from a second new one:
            # both answer a prompt, and only one of them is the restart path.
            (root / "resumed").write_text(session)
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
            # One line per process that got this far. A resumed session is a
            # fresh process, so the count is how many times this binding
            # configured a session — which is the only way a test can see that
            # a restored session was configured again rather than used as
            # Codex left it.
            with (root / "configured").open("a") as log:
                log.write(json.dumps(configured_steps) + "\n")
        result(msg["id"], configs())
    elif method == "session/prompt":
        pending = msg["id"]
        if mode == "terminal-command":
            update(command_tool_call())
            # Terminal output arrives as deltas, one frame per chunk, and the
            # completion then repeats the whole of it. More than one chunk,
            # because one chunk cannot show a later frame overwriting an
            # earlier one.
            for chunk in ("compiling\n", "2 passed\n"):
                update({"sessionUpdate": "tool_call_update", "toolCallId": "command-1",
                        "_meta": {"terminal_output_delta": {"data": chunk, "terminal_id": "command-1"}}})
            update({"sessionUpdate": "tool_call_update", "toolCallId": "command-1", "status": "completed",
                    "rawOutput": {"formatted_output": "compiling\n2 passed\n", "exit_code": 0},
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
