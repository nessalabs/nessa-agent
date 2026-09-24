"""ACP test handler for Opencode adapter tests; launched only by the test suite.

Which frames are recorded and which are not, because the difference decides
how much the tests below are worth:

RECORDED off Opencode 1.18.31, by running `opencode acp` with an empty home and
keeping what it answered:
  - the `initialize` result, including `agentInfo` and the absence of any
    steering or instruction capability;
  - `session/new`: it succeeds with no authentication challenge, and offers
    `model` and `mode` as config options, with `build` current;
  - that unknown parameters on `session/new` are ignored rather than refused;
  - the `available_commands_update` it schedules while answering `session/new`,
    before any configuration. The pinned source uses a zero-delay timer, so
    either wire order relative to the response is valid.

ASSUMED from the ACP specification, because observing them needs a model turn
and this environment's network policy does not allow OpenCode Zen's host:
  - `tool_call` and `tool_call_update` (the `read-tool-call` mode);
  - `session/request_permission` and its `rawInput` (the `edit-permission`
    mode), in particular that Opencode carries arguments there and needs no
    `_meta` fallback of the kind Codex required;
  - a mid-turn `current_mode_update` (`left-the-mode`, `announces-start-mode`).

An assumed shape that turns out wrong shows up as this handler and Opencode
disagreeing, not as a test that quietly passes — but until one has been seen,
the production code that reads them says "assumed", and so does this.

Adversarial modes, each naming the contract it deliberately breaks, since a
shared contract fixture otherwise conforms by default: `wrong-harness` (lies in
`agentInfo.name`), `wrong-version` (lies in `agentInfo.version`),
`model-not-offered` (omits the asked-for model), `model-refused` (errors the
model selection), `mode-refused` (reports a mode other than the one selected),
`left-the-mode` (leaves the selected mode mid-turn), `announces-unknown-mode`
(names a mode the session does not offer), `wrong-session-startup-update`
(races an advisory for another context), and
`execution-output-before-session-response` (races execution output before a
context has been admitted).
"""
import json
import os
import pathlib
import sys

mode = sys.argv[1]
root = pathlib.Path.cwd()
session = "ses_opencode" + str(os.getpid())

# The launch environment is the whole of what this binding says before the
# protocol starts, and for Opencode it carries the part that ACP cannot: the
# session mode is selected over the protocol, but the permission policy is
# configuration, so it is set here or it is not set at all. Asserted as parsed
# JSON rather than as a string, because Opencode skips an OPENCODE_PERMISSION
# it cannot parse with nothing but a warning — a policy that fails to parse is
# a session with no bound, and it would fail silently.
policy = json.loads(os.environ["OPENCODE_PERMISSION"])
assert policy["*"] == "deny", policy
assert policy["read"]["*"] == "allow", policy
assert policy["read"]["*.env"] == "deny", policy
# These next two are in tension with the line above, deliberately and not by
# oversight. `read` is asked for permission with the path it is about, so
# denying `*.env` there works; `grep` is asked with the regular expression
# instead and Opencode runs ripgrep with `--hidden`, so an allowed `grep`
# returns matches from the files `read` refuses to open. The `.env` rules stop
# the direct path and are not claimed to be a secrets boundary; `binding.rs`
# says so where the policy is defined, and this asserts the pair that makes it
# true rather than leaving the two lines to look like a contradiction.
for allowed in ("grep", "glob"):
    assert policy[allowed] == "allow", policy
# Nothing that acts, named one by one so that widening the policy has to be
# deliberate. `task` spawns a subagent with its own policy; the rest are the
# tools Opencode's own permission vocabulary exposes. MCP tools are not in this
# list and cannot be — their names come from whatever servers the host hands
# over at `session/new` — which is why the policy denies by default.
for denied in ("bash", "edit", "webfetch", "websearch", "task", "skill"):
    assert policy.get(denied, "deny") == "deny", policy
assert os.environ["OPENCODE_DISABLE_PROJECT_CONFIG"] == "1"
# The other half of the same bound. A plugin's tools are not subject to the
# permission policy at all, so the policy cannot be what stops them; this stops
# them being loaded.
assert os.environ["OPENCODE_PURE"] == "1"
# Two things a launch would otherwise do on its own: reach models.opencode.ai
# at startup and hourly after, and let the pinned binary replace itself. The
# first buys a read-and-plan session nothing, and the second would move the
# version this profile's `initialize` check is written against.
assert os.environ["OPENCODE_DISABLE_MODELS_FETCH"] == "1"
assert os.environ["OPENCODE_DISABLE_AUTOUPDATE"] == "1"
assert "CODEX_CONFIG" not in os.environ
assert os.environ["XDG_DATA_HOME"] != os.environ["NESSA_REFUSED_OPENCODE_DATA_HOME"]
assert os.environ["HOME"] != os.environ["NESSA_REFUSED_OPENCODE_HOME"]
assert pathlib.Path(os.environ["XDG_CONFIG_HOME"]).is_relative_to(pathlib.Path(os.environ["HOME"]))
assert pathlib.Path(os.environ["XDG_CACHE_HOME"]).is_relative_to(pathlib.Path(os.environ["HOME"]))
assert pathlib.Path(os.environ["XDG_STATE_HOME"]).is_relative_to(pathlib.Path(os.environ["HOME"]))
assert pathlib.Path(os.environ["XDG_DATA_HOME"]).is_relative_to(pathlib.Path(os.environ["HOME"]))
for alternate in ("OPENCODE_CONFIG", "OPENCODE_CONFIG_CONTENT", "OPENCODE_CONFIG_DIR"):
    assert alternate not in os.environ

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
    # Protocol-shape choices for exact option matching. Their names do not
    # assert current production eligibility, authentication, or service
    # acceptance; this fixture never contacts OpenCode Zen.
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
    return os.environ.get("NESSA_EXPECTED_OPENCODE_MODEL", "exact-fixture-model")


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
            # Protocol shape observed from the pinned ACP. This fixture does
            # not infer whether a production call requires or accepts a key.
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
        if mode == "startup-update-before-session-response":
            update({"sessionUpdate": "available_commands_update", "availableCommands": []})
        elif mode == "wrong-session-startup-update":
            send({"jsonrpc": "2.0", "method": "session/update", "params": {
                "sessionId": "another-session", "update": {
                    "sessionUpdate": "available_commands_update", "availableCommands": []}}})
        elif mode == "execution-output-before-session-response":
            update({"sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "too early"}})
        result(msg["id"], {"sessionId": session, **configs()})
        # Recorded: 1.18.31 sends this unprompted the moment it has answered
        # `session/new`, before anything has been configured. It is sent in
        # every mode here rather than behind one of them, because that is where
        # the real binary sends it and a path every test crosses is a path no
        # test can forget. An adapter that treated an advisory update as
        # execution output, or checked it against a session it has not admitted
        # yet, would fail `open()` on it — see the test named for it.
        if mode not in ("startup-update-before-session-response",
                        "wrong-session-startup-update",
                        "execution-output-before-session-response"):
            update({"sessionUpdate": "available_commands_update", "availableCommands": []})
    elif method == "session/set_config_option":
        params = msg["params"]
        assert params["sessionId"] == session
        configured_steps.append(params["configId"])
        if params["configId"] == "model":
            assert params["value"] == model_id()
            if mode == "startup-update-configuring":
                update({"sessionUpdate": "config_option_update", **configs()})
            # The truth, at the one moment it is the truth: nothing has
            # selected a mode yet, so the session really is in `build`.
            if mode == "announces-start-mode":
                update({"sessionUpdate": "current_mode_update",
                        "currentModeId": "build"})
            # Adversarial: a mode Opencode does not offer, announced in the
            # window where the two it does offer are tolerated. Breaks the
            # contract that `currentModeId` names one of the session's own
            # modes.
            if mode == "announces-unknown-mode":
                update({"sessionUpdate": "current_mode_update",
                        "currentModeId": "yolo"})
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
        elif mode in ("stall", "process-exit"):
            text("running")
            if mode == "process-exit":
                sys.exit(17)
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
