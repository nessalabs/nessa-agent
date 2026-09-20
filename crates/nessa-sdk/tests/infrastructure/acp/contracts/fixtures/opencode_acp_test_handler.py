"""ACP test handler for Opencode adapter tests; launched only by the test suite.

The shapes here were read off Opencode 1.18.31 itself, by running
`opencode acp` with an empty home and recording what it answered: the
`initialize` result, and a `session/new` that succeeds with no authentication
challenge and offers `model` and `mode` as config options.
"""
import json
import os
import pathlib
import sys

mode = sys.argv[1]
root = pathlib.Path.cwd()
session = "ses_opencode" + str(os.getpid())

# The launch environment is the whole of what this binding says before the
# protocol starts. Opencode takes no configuration variable of its own — unlike
# Codex it is configured entirely over ACP — so the only thing set is the one
# that keeps a gateway process from trying to open a browser.
assert os.environ["NO_BROWSER"] == "1"
assert "CODEX_CONFIG" not in os.environ

(root / "pid").write_text(str(os.getpid()))
# What Opencode itself opens a session on: its gateway's current default, which
# is not the model this binding asked for.
selected = "opencode/big-pickle"
current_mode = "build"
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
    # OpenCode Zen's free models, as the real `session/new` listed them.
    offered = ["opencode/big-pickle"] if mode == "model-not-offered" else [
        "opencode/big-pickle",
        "opencode/nemotron-3-ultra-free",
        model_id(),
    ]
    reported_mode = "build" if mode == "mode-refused" else current_mode
    return {"configOptions": [
        {"id": "model", "name": "Model", "category": "model", "type": "select",
         "currentValue": selected,
         "options": [{"value": value, "name": value} for value in offered]},
        {"id": "mode", "name": "Session Mode", "category": "mode", "type": "select",
         "currentValue": reported_mode,
         "options": [{"value": "build", "name": "Build"}, {"value": "plan", "name": "Plan"}]},
    ]}


def model_id():
    return "exact-fixture-model"


for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        name = "Opencode" if mode == "wrong-harness" else "OpenCode"
        version = "9.9.9" if mode == "wrong-version" else "1.18.31"
        result(msg["id"], {
            "protocolVersion": 1,
            "agentInfo": {"name": name, "version": version},
            "agentCapabilities": {
                "loadSession": True,
                "promptCapabilities": {"embeddedContext": True, "image": True},
                "sessionCapabilities": {"close": {}, "fork": {}, "list": {}, "resume": {}},
            },
            # Opencode offers a sign-in and does not require one: the free
            # models this binding is for are reached without it.
            "authMethods": [{"id": "opencode-login", "name": "Login with opencode",
                             "description": "Run `opencode auth login` in the terminal"}],
        })
    elif method in ("session/new", "session/resume"):
        params = msg["params"]
        # Opencode is told the workspace and the trusted MCP servers, and
        # nothing else: a model or a tool policy arriving here would be this
        # binding configuring a session Opencode does not configure that way.
        assert set(params) <= {"cwd", "mcpServers", "sessionId"}
        # Compared as resolved paths, for the reason the Codex handler gives.
        assert pathlib.Path(params["cwd"]).resolve() == root
        assert params["mcpServers"] == []
        if method == "session/resume":
            session = params["sessionId"]
        result(msg["id"], {"sessionId": session, **configs()})
    elif method == "session/set_config_option":
        params = msg["params"]
        assert params["sessionId"] == session
        configured_steps.append(params["configId"])
        if params["configId"] == "model":
            assert params["value"] == model_id()
            if mode == "startup-update-configuring":
                update({"sessionUpdate": "config_option_update", **configs()})
            if mode == "model-refused":
                send({"id": msg["id"], "error": {"code": -32042, "message": "unknown model"}})
                continue
            selected = params["value"]
        else:
            # The model is always settled first: a refused model must not leave
            # a session that has been put into a mode anyway.
            assert configured_steps == ["model", "mode"], configured_steps
            assert params["value"] == "plan"
            current_mode = params["value"]
        result(msg["id"], configs())
    elif method == "session/prompt":
        pending = msg["id"]
        if mode == "read-tool-call":
            # A tool call in the protocol's own shape, which is what Opencode
            # reports: no extension of its own to interpret.
            update({"sessionUpdate": "tool_call", "toolCallId": "read-1", "kind": "read",
                    "title": "Read src/main.rs", "status": "pending",
                    "rawInput": {"filePath": "src/main.rs"}})
            update({"sessionUpdate": "tool_call_update", "toolCallId": "read-1",
                    "status": "completed",
                    "content": [{"type": "content",
                                 "content": {"type": "text", "text": "fn main() {}"}}]})
        elif mode == "left-the-mode":
            # The session leaving the policy it was opened under, after the
            # fact — whoever changed it. The turn then finishes normally, so
            # that the only thing that can fail the execution is the update
            # itself: a handler that stopped answering would fail it on the
            # deadline instead, and the test would pass without the check.
            update({"sessionUpdate": "current_mode_update", "currentModeId": "build"})
            text(msg["params"]["prompt"][0]["text"])
        elif mode == "edit-permission":
            send({"id": "edit-review", "method": "session/request_permission", "params": {
                "sessionId": session,
                "toolCall": {"toolCallId": "edit-1", "kind": "edit", "status": "pending",
                             "title": "Edit src/main.rs",
                             "rawInput": {"filePath": "src/main.rs", "newText": "fn main() {}"}},
                "options": [
                    {"optionId": "allow_once", "kind": "allow_once", "name": "Allow Once"},
                    {"optionId": "reject_once", "kind": "reject_once", "name": "Reject"},
                ],
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
    elif msg.get("id") == "edit-review":
        outcome = msg["result"]["outcome"]
        (root / "permission-outcome").write_text(json.dumps(outcome))
        update({"sessionUpdate": "tool_call_update", "toolCallId": "edit-1", "status": "completed"})
        result(pending, {"stopReason": "end_turn"})
        pending = None
