"""Independent local MCP Events Beta peer. Python 3 standard library only."""
import argparse
import base64
import json
import socket
from http.server import BaseHTTPRequestHandler, HTTPServer

REVISION = "2026-07-28"
EXTENSION = "io.modelcontextprotocol/events"
EVENT = "ci.build_fínished"
STAMP = "2026-11-24T12:00:00Z"
LIMIT = 256 * 1024


def result(**fields):
    return {"resultType": "complete", **fields}


def descriptor(name, modes):
    return {
        "name": name,
        "description": "Synthetic results from a local project build feed",
        "delivery": modes,
        "inputSchema": {
            "type": "object",
            "properties": {"project": {"type": "string"}},
            "required": ["project"],
        },
        "payloadSchema": {
            "type": "object",
            "properties": {"project": {"type": "string"}, "outcome": {"type": "string"}},
            "required": ["project", "outcome"],
        },
    }


def occurrence(event_id, arguments, outcome):
    return {
        "eventId": event_id,
        "name": EVENT,
        "timestamp": STAMP,
        "data": {"project": arguments["project"], "outcome": outcome},
    }


class Server(HTTPServer):
    def __init__(self, port, profile, response_format):
        super().__init__(("127.0.0.1", port), Handler)
        self.profile = profile
        self.response_format = response_format


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self):
        super().setup()
        self.connection.settimeout(3)

    def do_POST(self):
        if self.path != "/mcp":
            self.send_error(404)
            return
        try:
            size = int(self.headers.get("Content-Length", "0"))
            if not 0 < size <= LIMIT:
                self.send_error(413)
                return
            message = json.loads(self.rfile.read(size))
        except (ValueError, socket.timeout):
            self.send_error(400)
            return
        rid = message.get("id")
        method = message.get("method")
        params = message.get("params", {})
        meta = params.get("_meta", {})
        accept = self.headers.get("Accept", "")
        standard = (
            message.get("jsonrpc") == "2.0"
            and self.headers.get("Mcp-Method") == method
            and self.headers.get("MCP-Protocol-Version") == REVISION
            and meta.get("io.modelcontextprotocol/protocolVersion") == REVISION
            and meta.get("io.modelcontextprotocol/clientCapabilities", {})
            .get("extensions", {})
            .get(EXTENSION)
            == {}
            and "application/json" in accept
            and "text/event-stream" in accept
            and "Mcp-Session-Id" not in self.headers
            and "Authorization" not in self.headers
        )
        if not standard:
            self.send_rpc(rid, error=(-32020, "unexpected transport metadata"), status=400)
            return

        poll = method == "events/poll"
        # Use an exact published-spec value rather than the client's encoder.
        encoded_event = "=?base64?" + base64.b64encode(EVENT.encode()).decode() + "?="
        if self.headers.get("Mcp-Name") != (encoded_event if poll else None):
            self.send_rpc(rid, error=(-32020, "unexpected event routing name"), status=400)
            return

        if method == "server/discover":
            self.send_rpc(
                rid,
                result(
                    supportedVersions=[REVISION],
                    capabilities={"extensions": {EXTENSION: {}}},
                    _meta={"io.modelcontextprotocol/serverInfo": {"name": "ci-fixture", "version": "1.0"}},
                ),
            )
        elif method == "events/list" and params.get("cursor") is None:
            self.send_rpc(rid, result(events=[descriptor("ci.other", ["poll"])], nextCursor="page-2"))
        elif method == "events/list" and params["cursor"] == "page-2":
            delivery = ["push"] if self.server.profile == "push-only" else ["poll"]
            self.send_rpc(rid, result(events=[descriptor(EVENT, delivery)]))
        elif poll and self.server.profile == "replay" and params.get("name") == EVENT:
            self.poll(rid, params)
        elif poll and self.server.profile == "push-only":
            self.send_rpc(rid, error=(-32026, "Poll is unsupported"))
        else:
            self.send_rpc(rid, error=(-32601, "Method not found"))

    def poll(self, rid, params):
        arguments = params.get("arguments", {})
        cursor = params.get("cursor")
        if not isinstance(arguments.get("project"), str) or params.get("maxEvents", 0) < 1:
            self.send_rpc(rid, error=(-32602, "Project required"))
            return
        replies = {
            None: ([], "retention-a-0", False),
            "retention-a-0": ([occurrence("build-1", arguments, "passed")], "retention-a-1", False),
            "retention-a-1": ([], "retention-b-0", True),
            "retention-b-0": ([occurrence("build-2", arguments, "failed")], "retention-b-1", False),
            "retention-b-1": ([], "retention-b-1", False),
        }
        events, position, gap = replies.get(cursor, ([], "retention-b-0", True))
        self.send_rpc(
            rid, result(events=events, cursor=position, truncated=gap, hasMore=False, nextPollMs=1000)
        )

    def send_rpc(self, rid, value=None, *, error=None, status=200):
        envelope = {"jsonrpc": "2.0", "id": rid}
        if error is None:
            envelope["result"] = value
        else:
            envelope["error"] = {"code": error[0], "message": error[1]}
        payload = json.dumps(envelope, ensure_ascii=False, separators=(",", ":")).encode()
        sse = self.server.response_format == "sse" and status == 200
        if sse:
            payload = b": synthetic CI event feed\r\n\r\ndata: " + payload + b"\r\n\r\n"
        self.send_response(status)
        self.send_header("Content-Type", "text/event-stream" if sse else "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Connection", "close")
        self.end_headers()
        if sse:
            # Small writes exercise the socket reader; unit tests force parser fragmentation.
            for offset in range(0, len(payload), 43):
                self.wfile.write(payload[offset : offset + 43])
        else:
            self.wfile.write(payload)
        self.close_connection = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=("replay", "push-only"), default="replay")
    parser.add_argument("--format", choices=("json", "sse"), default="json")
    parser.add_argument("--port", default=0, type=int, help="zero chooses an ephemeral loopback port")
    parser.add_argument("--ready-json", action="store_true", help="emit a structured test readiness record")
    args = parser.parse_args()
    with Server(args.port, args.profile, args.format) as server:
        if args.ready_json:
            print(
                json.dumps(
                    {"fixture": "mcp-events-beta-v1", "profile": args.profile,
                     "format": args.format, "port": server.server_port}
                ),
                flush=True,
            )
        else:
            print(f"http://127.0.0.1:{server.server_port}/mcp", flush=True)
        server.serve_forever()


if __name__ == "__main__":
    main()
