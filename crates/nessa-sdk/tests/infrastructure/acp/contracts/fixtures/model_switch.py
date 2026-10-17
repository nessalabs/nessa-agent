"""Deterministic native-boundary model control fixture; no model/service calls."""
import json
import os
import sys

profile = sys.argv[1]
version = "0.76.0" if profile == "claude" else "1.12.0"
model = os.environ.get("ANTHROPIC_MODEL", "exact-fixture-model")
mode = "default" if profile == "claude" else "read-only"
session = "held-native-context"
active = None
prompts = 0
adjust = "adjust" in sys.argv[2:]
steering = "steer" in sys.argv[2:]
steers = 0
effort = "high" if adjust else None

def send(value):
    print(json.dumps(value), flush=True)

def result(id, value):
    send({"jsonrpc": "2.0", "id": id, "result": value})

def options():
    values = [{"id": "model", "currentValue": model, "options": [{"value": value} for value in (model, "gpt-6-astra", "gpt-5.6-terra", "selected-model")]}, {"id": "mode", "currentValue": mode}]
    if effort is not None:
        values.append({"id": "effort" if profile == "claude" else "reasoning_effort", "category": "thought_level", "currentValue": effort, "options": [{"value": x} for x in ("medium", "high")]})
    return {"configOptions": values}

def observed():
    if steering:
        with open("model-steering-observed", "w") as record:
            json.dump({"steers": steers, "prompts": prompts}, record)

def text(value):
    send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": session, "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": value}}}})

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    params = message.get("params", {})
    id = message.get("id")
    if method == "initialize":
        result(id, {"_meta": {"steering": {"supported": steering}}, "protocolVersion": 1, "agentInfo": {"name": "claude-code-acp" if profile == "claude" else "@agentclientprotocol/codex-acp", "version": version}, "agentCapabilities": {"sessionCapabilities": {"resume": {}}}})
    elif method in ("session/new", "session/resume"):
        session = params.get("sessionId", session)
        result(id, {"sessionId": session, **options()})
    elif method == "session/set_config_option":
        assert params["sessionId"] == session
        if params["configId"] == "model":
            model = params["value"]
            if adjust and model in ("claude-haiku-4-5-20251001", "gpt-5.6-terra"):
                mode = "default" if profile == "claude" else "read-only"
                effort = None if profile == "claude" else "medium"
        elif params["configId"] == "mode":
            mode = params["value"]
        elif params["configId"] in ("effort", "reasoning_effort"):
            effort = params["value"]
        else:
            raise AssertionError(params)
        result(id, options())
        if active is not None and not steering:
            # The existing native prompt settles after the configuration receipt.
            text("old turn preserved")
            result(active, {"stopReason": "end_turn"})
            active = None
    elif method == "session/prompt":
        prompts += 1
        observed()
        text("native turn running")
        if prompts == 1:
            active = id
        else:
            result(id, {"stopReason": "end_turn"})
    elif method == "_session/steering":
        assert steering and active is not None
        steers += 1
        observed()
        result(id, {"outcome": "injected"})
        text("native steer accepted")
        result(active, {"stopReason": "end_turn"})
        active = None
    elif method == "session/close":
        result(id, {})
    elif method == "shutdown":
        result(id, {})
    elif method == "exit":
        break
    else:
        raise AssertionError(message)

observed()
