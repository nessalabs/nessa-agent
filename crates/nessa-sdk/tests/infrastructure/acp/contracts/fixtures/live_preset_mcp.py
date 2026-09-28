"""Harmless MCP side-effect witness for opt-in live approval probes."""
import json
import pathlib
import sys

log = pathlib.Path(sys.argv[1])
for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    method = message.get("method")
    if method == "initialize":
        result = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}},
                  "serverInfo": {"name": "probe", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "record_probe", "description": "Append a harmless test marker to the isolated probe log.",
                            "inputSchema": {"type": "object", "properties": {"marker": {"type": "string"}},
                                            "required": ["marker"], "additionalProperties": False}}]}
    elif method == "tools/call" and message["params"]["name"] == "record_probe":
        with log.open("a") as output:
            output.write(json.dumps(message["params"]["arguments"]) + "\n")
        result = {"content": [{"type": "text", "text": "MCP_RECORDED_239"}]}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"],
                          "error": {"code": -32601, "message": "Unknown method"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
