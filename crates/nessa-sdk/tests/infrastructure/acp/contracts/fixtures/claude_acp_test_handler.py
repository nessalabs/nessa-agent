"""ACP test handler for Claude adapter tests; launched only by the test suite."""
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
def prompt_text(blocks):
    # A message of images alone has no text block.
    return next((block["text"] for block in blocks if block["type"] == "text"), "")
def record(name, value):
    temporary = root / (name + ".tmp")
    temporary.write_text(value)
    temporary.replace(root / name)

if mode == "identity-credentials":
    assert os.environ["NESSA_FIXTURE_CREDENTIAL"] in ("synthetic-secret-one", "synthetic-secret-two")
    assert os.environ["CLAUDE_CONFIG_DIR"] == "/fixture/config-a"
    record("credential-received", "yes")

record("pid", session)
launches_path = root / "launches"
launches = json.loads(launches_path.read_text()) if launches_path.exists() else []
launches.append(os.getpid())
record("launches", json.dumps(launches))
history = []
pending = None
child = None
permission_id = "provider-permission"
provider_cancel_mode = mode in ("permission-provider-cancel", "permission-provider-cancel-string")
cancelled_permission_id = "cancelled-review" if mode.endswith("-string") else 42

def send(value):
    print(json.dumps({"jsonrpc": "2.0", **value}), flush=True)

def result(id, value):
    send({"id": id, "result": value})

def configs(selected=model, permission_mode="default"):
    return {"configOptions": [{"id": "model", "currentValue": selected}, {"id": "mode", "currentValue": permission_mode}]}

def duplicate_configs(id):
    response = configs()
    response["configOptions"].append({"id": id, "currentValue": "bypassPermissions" if id == "mode" else "alias"})
    return response


def update(value, sid=None):
    send({"method": "session/update", "params": {"sessionId": session if sid is None else sid, "update": value}})

def text(value="hello", sid=None):
    update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": value}}, sid)

def tool(name="Write"):
    return {"toolCallId": "file-1", "title": "Write fixture.txt", "kind": "edit", "status": "pending",
            "_meta": {"claudeCode": {"toolName": name}}, "rawInput": {"file_path": str(root / "fixture.txt"), "content": "fixture"}}

if mode == "startup-stall":
    time.sleep(20)
    sys.exit(0)
if mode in ("slow-launch", "slow-launch-session-stall"):
    # Stands in for the operating system scanning a freshly written runtime on
    # its first execution: nothing is read or written until this passes, so the
    # delay lands entirely before the child answers anything.
    time.sleep(5)
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
        if mode == "startup-update-before-initialize":
            update({"sessionUpdate": "current_mode_update", "currentModeId": "default"})
        assert msg["params"]["clientCapabilities"]["terminal"] is False
        result(msg["id"], {"protocolVersion": 1, "agentInfo": {"version": "wrong" if mode == "wrong-version" else "0.76.0"}, "agentCapabilities": {"promptCapabilities": {"image": "image" in mode}, "sessionCapabilities": {} if (mode == "resume-unsupported" or (mode == "steering-resume-removed" and (root / "saved-session").exists())) else {"resume": {}}}, "_meta": {"steering": {"supported": mode.startswith("steering") and mode != "steering-unsupported" and not (mode == "steering-capabilities-change" and (root / "saved-session").exists())}}})
    elif method in ("session/new", "session/resume"):
        if mode == "startup-update-before-session":
            update({"sessionUpdate": "current_mode_update", "currentModeId": "default"})
        saved = root / "saved-session"
        if method == "session/resume":
            assert saved.exists(), "cannot restore missing session"
            state = json.loads(saved.read_text())
            assert msg["params"]["sessionId"] == state["id"]
            session = state["id"]
            history = state["history"]
            record("resume-observed", session)
            if mode == "resume-gated":
                while not (root / "resume-healthy").exists():
                    time.sleep(0.005)
            if mode == "resume-stall":
                time.sleep(20)
                continue
            if mode == "resume-failure" or (mode == "resume-retry" and not (root / "resume-healthy").exists()):
                send({"id": msg["id"], "error": {"code": -32000, "message": "restore failed"}})
                continue
            if mode == "resume-wrong-id":
                result(msg["id"], {"sessionId": "different-session", **configs()})
                continue
        else:
            if mode in ("new-session-stall", "slow-launch-session-stall"):
                record("new-session-wait", session)
                time.sleep(20)
                continue
            count_path = root / "new-session-count"
            count = int(count_path.read_text()) if count_path.exists() else 0
            record("new-session-count", str(count + 1))
            record("saved-session", json.dumps({"id": session, "history": history}))
        if mode == "system-prompt":
            assert msg["params"]["_meta"]["systemPrompt"] == "Core instructions.\nPlugin instructions."
            assert set(msg["params"]["_meta"]) == {"systemPrompt", "claudeCode"}
        else:
            assert "systemPrompt" not in msg["params"]["_meta"]
        options = msg["params"]["_meta"]["claudeCode"]["options"]
        assert options["model"] == model
        assert options["settingSources"] == []
        assert options["tools"] == {"type": "preset", "preset": "claude_code"}
        assert "Bash" in options["disallowedTools"]
        assert "Bash" in options["settings"]["permissions"]["deny"]
        for canonical in ("TaskOutput", "TaskStop"):
            assert canonical in options["disallowedTools"]
            assert canonical in options["settings"]["permissions"]["deny"]
        for historical in ("BashOutput", "KillShell"):
            assert historical not in options["disallowedTools"]
            assert historical not in options["settings"]["permissions"]["deny"]
        # Every tool the harness offers is reviewed by Nessa's permission owner;
        # naming them one by one left unnamed tools running unreviewed and
        # failed the execution when one of them was called.
        assert options["settings"]["permissions"]["ask"] == ["*"]
        assert options["settings"]["disableAllHooks"] is True
        assert options["settings"]["allowedMcpServers"] == []
        response = configs("alias" if mode == "wrong-model" else model)
        if mode.startswith("duplicate-session-"):
            response = duplicate_configs(mode.removeprefix("duplicate-session-"))
        if method == "session/new" or mode != "resume-no-id":
            response["sessionId"] = "s" * 257 if mode == "oversized-session-id" else session
        result(msg["id"], response)
    elif method == "session/set_config_option":
        if mode in ("startup-permission-missing-id", "startup-permission-null-id"):
            request = {"method": "session/request_permission", "params": {"sessionId": session}}
            if mode == "startup-permission-null-id":
                request["id"] = None
            send(request)
        elif mode == "startup-update-mode":
            update({"sessionUpdate": "current_mode_update", "currentModeId": "bypassPermissions"})
        elif mode == "startup-update-model":
            update({"sessionUpdate": "config_option_update", **configs("alias")})
        elif mode.startswith("startup-update-duplicate-"):
            update({"sessionUpdate": "config_option_update", **duplicate_configs(mode.removeprefix("startup-update-duplicate-"))})
        elif mode == "startup-update-wrong-session":
            update({"sessionUpdate": "current_mode_update", "currentModeId": "default"}, "other-session")
        elif mode == "startup-update-output":
            text("unsolicited startup output")
        elif mode == "startup-update-mode-option":
            # The mode this binding is in the act of setting, reported while the
            # request that sets it is still in flight.
            update({"sessionUpdate": "config_option_update", **configs(permission_mode="bypassPermissions")})
        elif mode == "startup-update-valid":
            update({"sessionUpdate": "config_option_update", **configs()})
            update({"sessionUpdate": "current_mode_update", "currentModeId": "default"})
            update({"sessionUpdate": "available_commands_update", "availableCommands": []})
        assert msg["params"]["value"] == "default"
        if mode == "configuration-stall" or (mode == "configuration-stall-on-resume" and len(launches) > 1):
            record("configuration-wait", session)
            time.sleep(20)
            continue
        if mode == "configuration-error":
            send({"id": msg["id"], "error": {"code": -32042, "message": "configuration failed"}})
            continue
        response = configs(permission_mode="bypassPermissions" if mode == "wrong-mode" else "default")
        if mode.startswith("duplicate-config-"):
            response = duplicate_configs(mode.removeprefix("duplicate-config-"))
        result(msg["id"], response)
        if mode == "blocked-prompt-write":
            signal.pause()
        if mode == "idle-config-change" or (mode == "idle-config-change-once" and len(launches) == 1):
            update({"sessionUpdate": "config_option_update", **configs("alias")})
    elif method == "session/prompt":
        assert pending is None
        if mode == "system-prompt":
            assert set(msg["params"]) == {"sessionId", "prompt"}
            assert msg["params"]["sessionId"] == session
            assert len(msg["params"]["prompt"]) == 1
            assert msg["params"]["prompt"][0]["type"] == "text"
            assert msg["params"]["prompt"][0]["text"] in ("first user message", "second user message")
        pending = msg["id"]
        record("prompt-observed", json.dumps(msg["params"]["prompt"]))
        user_text = prompt_text(msg["params"]["prompt"])
        prior_history = history[:]
        history.append(user_text)
        record("saved-session", json.dumps({"id": session, "history": history}))
        if mode.startswith("steering"):
            text("running:" + user_text)
        elif mode in ("permission-missing-id", "permission-null-id"):
            request = {"method": "session/request_permission", "params": {
                "sessionId": session, "toolCall": tool(), "options": [
                    {"optionId": "deny-one", "kind": "reject_once", "name": "Deny once"}]}}
            if mode == "permission-null-id":
                request["id"] = None
            send(request)
        elif mode == "duplicate-json-mode":
            print('{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":' + json.dumps(session) + ',"update":{"sessionUpdate":"current_mode_update","currentModeId":"bypassPermissions","currentModeId":"default"}}}', flush=True)
        elif mode == "malformed":
            print("{broken", flush=True)
        elif mode == "oversize":
            print("x" * 20000, flush=True)
        elif mode == "provider-error" or (mode == "provider-error-once" and len(launches) == 1):
            if mode == "provider-error-once":
                text("retained-before-failure")
            send({"id": pending, "error": {"code": -32000, "message": "provider plan does not allow this request"}})
        elif mode == "unsupported-tool-content":
            update({"sessionUpdate": "tool_call", **tool(), "content": [
                {"type": "content", "content": {"type": "text", "text": "before"}},
                {"type": "content", "content": {"type": "image", "data": "i" * 3000,
                                                    "mimeType": "image/png"}},
                {"type": "terminal", "terminalId": "command"},
                {"type": "bash_code_execution_result", "stdout": "opaque"},
            ]})
            opaque = [
                {"type": "future_result", "payload": "z" * 180, "ordinal": index}
                for index in range(12)
            ]
            update({"sessionUpdate": "tool_call_update", "toolCallId": "file-1",
                    "status": "completed", "content": opaque + [
                        {"type": "content", "content": {"type": "text", "text": "after"}}
                    ]})
            text("continued:" + user_text)
            result(pending, {"stopReason": "end_turn"})
            pending = None
        elif mode == "wrong-session":
            text(sid="someone-else")
        elif mode == "unknown-reason":
            result(pending, {"stopReason": "new-value"})
        elif mode.startswith("live-duplicate-"):
            update({"sessionUpdate": "config_option_update", **duplicate_configs(mode.removeprefix("live-duplicate-"))})
        elif mode == "config-change":
            update({"sessionUpdate": "config_option_update", **configs("alias")})
        elif mode == "flood":
            for _ in range(10000):
                text("flood")
        elif mode == "eof":
            sys.exit(0)
        elif mode in ("permission-pair", "permission-pair-write-failure"):
            if mode == "permission-pair-write-failure":
                os.close(sys.stdin.fileno())
            update({"sessionUpdate": "tool_call", **tool()})
            for review_id in ("first-review", "second-review"):
                send({"id": review_id, "method": "session/request_permission", "params": {
                    "sessionId": session, "toolCall": tool(), "options": [
                        {"optionId": "deny-one", "kind": "reject_once", "name": "Deny once"}]}})
            if mode == "permission-pair-write-failure":
                signal.pause()
        elif provider_cancel_mode:
            update({"sessionUpdate": "tool_call", **tool()})
            for review_id in (permission_id, cancelled_permission_id):
                send({"id": review_id, "method": "session/request_permission", "params": {
                    "sessionId": session, "toolCall": tool(), "options": [
                        {"optionId": "approve-one", "kind": "allow_once", "name": "Allow once"}]}})
            for review_id in (cancelled_permission_id, cancelled_permission_id, "unknown"):
                send({"method": "$/cancel_request", "params": {"requestId": review_id}})
            text("provider cancellation processed")
        elif mode in ("permission", "permission-write-failure", "permission-stop", "permission-failure", "permission-finished", "permission-provider-error", "permission-byte-flood", "unknown-tool"):
            update({"sessionUpdate": "tool_call", **tool("Bash" if mode == "unknown-tool" else "Write")})
            if mode == "permission-write-failure":
                os.close(sys.stdin.fileno())
            send({"id": permission_id, "method": "session/request_permission", "params": {
                "sessionId": session, "toolCall": {key: value for key, value in tool().items() if key != "title"}, "options": [
                    {"optionId": "approve-one", "kind": "allow_once", "name": "Allow once"},
                    {"optionId": "deny-one", "kind": "reject_once", "name": "Deny once"},
                    {"optionId": "never-choose", "kind": "allow_always", "name": "Always"}]}})
            if mode == "permission-write-failure":
                signal.pause()
            if mode == "permission-byte-flood":
                for _ in range(12):
                    text("x" * (3 * 1024 * 1024))
                result(pending, {"stopReason": "end_turn"})
                pending = None
            if mode == "permission-failure":
                print("{broken", flush=True)
            elif mode == "permission-finished":
                result(pending, {"stopReason": "end_turn"})
                pending = None
            elif mode == "permission-provider-error":
                send({"id": pending, "error": {"code": -32000, "message": "fixture provider failure"}})
        elif mode.startswith("declined-"):
            # A review this binding will not put to a host. The call is still
            # observed, the review is answered "no", and the turn finishes —
            # which is the point: refusing one tool is not refusing the turn.
            call = {key: value for key, value in tool("Bash").items() if key != "title"}
            options = [
                {"optionId": "approve-one", "kind": "allow_once", "name": "Allow once"},
                {"optionId": "deny-one", "kind": "reject_once", "name": "Deny once"}]
            if mode == "declined-tool":
                update({"sessionUpdate": "tool_call", **tool("Bash")})
            elif mode == "declined-options":
                # Nothing offerable survives filtering, so there was never a
                # decision a host could have made.
                update({"sessionUpdate": "tool_call", **tool("Write")})
                call = {key: value for key, value in tool().items() if key != "title"}
                options = [{"optionId": "always", "kind": "allow_always", "name": "Always"}]
            elif mode == "declined-write-failure":
                # Nothing can be written back: the refusal cannot reach the
                # agent, and that has to be recorded as what it is.
                update({"sessionUpdate": "tool_call", **tool("Bash")})
                os.close(sys.stdin.fileno())
            elif mode == "declined-divergent-name":
                # Observed as one tool, reviewed as another. The refusal follows
                # what was observed; the recorded name is the provider's claim.
                update({"sessionUpdate": "tool_call", **tool("Bash")})
                call["_meta"] = {"claudeCode": {"toolName": "Read"}}
            elif mode == "declined-unreadable":
                # Reviewed under an identity that was never observed: nothing
                # this binding could describe to somebody deciding.
                update({"sessionUpdate": "tool_call", **tool("Write")})
                call = {key: value for key, value in tool().items() if key != "title"}
                call["toolCallId"] = "never-observed"
            send({"id": permission_id, "method": "session/request_permission", "params": {
                "sessionId": session, "toolCall": call, "options": options}})
            if mode == "declined-write-failure":
                signal.pause()
        elif mode in ("stall", "image-stall", "complete-on-stop", "ignore-stop", "late-tool-close", "consumer-loss-during-close"):
            if mode == "late-tool-close":
                update({"sessionUpdate": "tool_call", **tool()})
            if mode == "ignore-stop":
                child = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(20)"])
                record("child-pid", str(child.pid))
            text("running")
        elif mode == "unknown-request":
            send({"id": "unsupported", "method": "terminal/create", "params": {"sessionId": session}})
        else:
            if mode == "message-identities":
                for identity, value in [("m1", "First "), ("m1", "reply."), ("m2", "Second reply.")]:
                    update({"sessionUpdate": "agent_message_chunk", "messageId": identity, "content": {"type": "text", "text": value}})
            elif mode == "empty-message-identity":
                update({"sessionUpdate": "agent_message_chunk", "messageId": "", "content": {"type": "text", "text": "invalid"}})
            elif mode == "byte-generation":
                for _ in range(4):
                    text("x" * (3 * 1024 * 1024))
            else:
                text("|".join(prior_history + [user_text]) if mode in ("resume-context", "resume-no-id") else user_text)
            stop_reason = "cancelled" if mode == "cancelled-once" and len(launches) == 1 else mode if mode in ("max_tokens", "max_turn_requests", "refusal", "cancelled") else "end_turn"
            result(pending, {"stopReason": stop_reason})
            pending = None
    elif method == "_session/steering":
        assert msg["params"]["sessionId"] == session
        assert msg["params"]["_meta"] == {"steering": {"idleBehavior": "promptRequired"}}
        assert pending is not None
        count_path = root / "steering-count"
        count = int(count_path.read_text()) if count_path.exists() else 0
        record("steering-count", str(count + 1))
        record("steering-observed", json.dumps(msg["params"]["prompt"]))
        if mode == "steering-transport":
            sys.exit(0)
        elif mode == "steering-malformed":
            result(msg["id"], {"outcome": "promptRequired", "reason": "unverified"})
        elif mode == "steering-detached":
            result(msg["id"], {"outcome": "startedNewTurn"})
        elif mode == "steering-wrong-id":
            result(msg["id"] + 1, {"outcome": "injected"})
        elif mode == "steering-error":
            send({"id": msg["id"], "error": {"code": -32001, "message": "ambiguous"}})
        elif mode in ("steering-stall", "steering-image-stall"):
            pass
        elif mode == "steering-required":
            result(pending, {"stopReason": "end_turn"})
            pending = None
            result(msg["id"], {"outcome": "promptRequired", "reason": "noRunningTurn"})
        else:
            result(msg["id"], {"outcome": "injected"})
            text("steered:" + prompt_text(msg["params"]["prompt"]))
            result(pending, {"stopReason": "end_turn"})
            pending = None
    elif method == "session/cancel":
        (root / "cancel-observed").write_text("yes")
        if pending is not None and mode not in ("ignore-stop", "consumer-loss-during-close"):
            if mode == "late-tool-close":
                update({"sessionUpdate": "tool_call_update", "toolCallId": "file-1", "status": "completed", "content": []})
                update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "late message"}})
                update({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "late thought"}})
                update({"sessionUpdate": "current_mode_update", "currentModeId": "bypassPermissions"})
            result(pending, {"stopReason": "end_turn" if mode == "complete-on-stop" else "cancelled"})
            pending = None
    elif mode == "permission-pair" and msg.get("id") in ("first-review", "second-review"):
        record(msg["id"] + "-outcome", json.dumps(msg["result"]["outcome"]))
    elif mode.startswith("declined-") and msg.get("id") == permission_id:
        # Every answer for this review, in order: one refusal must not be able
        # to hide behind a later one.
        seen = root / "permission-outcomes"
        prior = seen.read_text() if seen.exists() else ""
        record("permission-outcomes", prior + json.dumps(msg["result"]["outcome"]) + "\n")
        record("permission-outcome", json.dumps(msg["result"]["outcome"]))
        text("declined and carried on")
        result(pending, {"stopReason": "end_turn"})
        pending = None
    elif msg.get("id") == permission_id:
        choice = msg["result"]["outcome"]
        if provider_cancel_mode:
            assert (root / "provider-cancel-response").exists()
            send({"method": "$/cancel_request", "params": {"requestId": permission_id}})
        (root / "permission-outcome").write_text(json.dumps(choice))
        if choice["outcome"] == "selected":
            assert choice["optionId"] in ("approve-one", "deny-one")
            if choice["optionId"] == "approve-one":
                (root / "fixture.txt").write_text("fixture")
            update({"sessionUpdate": "tool_call_update", "toolCallId": "file-1", "status": "completed", "content": []})
            result(pending, {"stopReason": "end_turn"})
            pending = None
    elif provider_cancel_mode and msg.get("id") == cancelled_permission_id:
        assert not (root / "provider-cancel-response").exists(), "duplicate cancellation response"
        assert msg["result"]["outcome"]["outcome"] == "cancelled"
        (root / "provider-cancel-response").write_text("cancelled")
    elif msg.get("id") == "unsupported":
        assert msg["error"]["code"] == -32601
        result(pending, {"stopReason": "end_turn"})
        pending = None

if mode == "ignore-stop":
    # Keep ownership of/reap the child after TERM; force kills the entire group.
    time.sleep(20)
