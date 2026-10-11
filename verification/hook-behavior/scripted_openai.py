#!/usr/bin/env python3
"""Small OpenAI-compatible server for the Codex and OpenCode hook runs.

It speaks POST /v1/responses and POST /v1/chat/completions. The prompt carries
NESSA_HOOK_ROW and NESSA_SENTINEL. A later request that already contains a
tool result gets a final text reply. Every body is written under the directory
given on the command line. Data for the runner is the port number on stdout.
"""

import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ORIGINAL = "hook-sentinel-original"


def texts(value, found):
    if isinstance(value, str):
        found.append(value)
    elif isinstance(value, dict):
        for item in value.values():
            texts(item, found)
    elif isinstance(value, list):
        for item in value:
            texts(item, found)


def marker(body, name):
    chunks = []
    texts(body, chunks)
    prefix = name + "="
    for chunk in chunks:
        start = chunk.find(prefix)
        if start < 0:
            continue
        rest = chunk[start + len(prefix) :]
        token = []
        for character in rest:
            if character.isspace() or character in "\"'`<>{}[],":
                break
            token.append(character)
        if token:
            return "".join(token)
    return ""


def tool_names(value, found):
    if isinstance(value, dict):
        function = value.get("function")
        if isinstance(function, dict) and isinstance(function.get("name"), str):
            found.append(function["name"])
        elif value.get("type") == "function" and isinstance(value.get("name"), str):
            found.append(value["name"])
        for item in value.values():
            tool_names(item, found)
    elif isinstance(value, list):
        for item in value:
            tool_names(item, found)


def has_tool_result(value):
    if isinstance(value, dict):
        if value.get("type") == "function_call_output":
            return True
        if value.get("role") in ("tool", "function"):
            return True
        return any(has_tool_result(item) for item in value.values())
    if isinstance(value, list):
        return any(has_tool_result(item) for item in value)
    return False


def pick_tool(names, row):
    def first(needles):
        for name in names:
            lowered = name.lower()
            if any(needle in lowered for needle in needles):
                return name
        return ""

    if row == "events-subagent":
        return first(("task", "agent", "subagent"))
    if row in ("events-plain", "events-compact", "events-resume"):
        return ""
    return first(("shell", "bash", "exec", "read"))


class Handler(BaseHTTPRequestHandler):
    request_dir = Path(".")
    counter = 0

    def log_message(self, fmt, *args):
        print(f"scripted-openai: {self.address_string()} {fmt % args}", file=sys.stderr)

    def _send(self, status, payload, content_type="application/json"):
        body = payload if isinstance(payload, bytes) else json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_sse(self, events):
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.end_headers()
        for name, data in events:
            self.wfile.write(f"event: {name}\ndata: {json.dumps(data)}\n\n".encode())
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def _send_sse_data(self, chunks):
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.end_headers()
        for chunk in chunks:
            self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def do_GET(self):
        if self.path.startswith("/v1/models"):
            self._send(
                200,
                {
                    "object": "list",
                    "data": [{"id": "scripted", "object": "model", "owned_by": "scripted"}],
                },
            )
            return
        self._send(404, {"error": "not found"})

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        raw = self.rfile.read(length)
        Handler.counter += 1
        path = self.request_dir / f"req-{Handler.counter:04d}.json"
        path.write_bytes(raw)
        try:
            body = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            self._send(400, {"error": "malformed json"})
            return
        if self.path.rstrip("/").endswith("/chat/completions"):
            payload = chat_response(body)
            if body.get("stream"):
                self._send_sse_data(chat_stream(payload))
                return
            self._send(200, payload)
            return
        if self.path.rstrip("/").endswith("/responses"):
            payload = responses_response(body)
            if body.get("stream"):
                self._send_sse(stream_events(payload))
                return
            self._send(200, payload)
            return
        self._send(404, {"error": "not found", "path": self.path})


def command_for(body):
    sentinel = marker(body, "NESSA_SENTINEL")
    if not sentinel:
        return "printf NESSA_NO_SENTINEL"
    return f"touch {sentinel}/{ORIGINAL}"


def tool_arguments(body):
    command = command_for(body)
    return json.dumps({"cmd": command, "command": command})


def stream_events(response):
    created = dict(response)
    created["status"] = "in_progress"
    created["output"] = []
    events = [
        ("response.created", {"type": "response.created", "response": created}),
        ("response.in_progress", {"type": "response.in_progress", "response": created}),
    ]
    for index, item in enumerate(response.get("output", [])):
        events.append(
            (
                "response.output_item.added",
                {
                    "type": "response.output_item.added",
                    "output_index": index,
                    "item": dict(item, status="in_progress"),
                },
            )
        )
        if item.get("type") == "message":
            text = item["content"][0]["text"]
            events.append(
                (
                    "response.output_text.delta",
                    {
                        "type": "response.output_text.delta",
                        "output_index": index,
                        "content_index": 0,
                        "item_id": item["id"],
                        "delta": text,
                    },
                )
            )
            events.append(
                (
                    "response.output_text.done",
                    {
                        "type": "response.output_text.done",
                        "output_index": index,
                        "content_index": 0,
                        "item_id": item["id"],
                        "text": text,
                    },
                )
            )
        elif item.get("type") == "function_call":
            events.append(
                (
                    "response.function_call_arguments.delta",
                    {
                        "type": "response.function_call_arguments.delta",
                        "output_index": index,
                        "item_id": item["id"],
                        "delta": item["arguments"],
                    },
                )
            )
            events.append(
                (
                    "response.function_call_arguments.done",
                    {
                        "type": "response.function_call_arguments.done",
                        "output_index": index,
                        "item_id": item["id"],
                        "arguments": item["arguments"],
                    },
                )
            )
        events.append(
            (
                "response.output_item.done",
                {"type": "response.output_item.done", "output_index": index, "item": item},
            )
        )
    events.append(("response.completed", {"type": "response.completed", "response": response}))
    return events


def responses_response(body):
    row = marker(body, "NESSA_HOOK_ROW") or "events-plain"
    phase = marker(body, "NESSA_PHASE")
    if has_tool_result(body) or phase == "second":
        text = "RESUME_OK" if phase == "second" else "TURN_DONE"
        return message_response(text)
    names = []
    tool_names(body.get("tools", []), names)
    chosen = pick_tool(names, row)
    if chosen:
        return {
            "id": f"resp_{Handler.counter}",
            "object": "response",
            "status": "completed",
            "model": "scripted",
            "output": [
                {
                    "type": "function_call",
                    "id": f"fc_{Handler.counter}",
                    "call_id": f"call_{Handler.counter}",
                    "name": chosen,
                    "arguments": tool_arguments(body),
                    "status": "completed",
                }
            ],
            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
        }
    text = "RESUME_OK" if phase == "second" else "PLAIN_OK"
    return message_response(text)


def message_response(text):
    return {
        "id": f"resp_{Handler.counter}",
        "object": "response",
        "status": "completed",
        "model": "scripted",
        "output": [
            {
                "id": f"msg_{Handler.counter}",
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [{"type": "output_text", "text": text, "annotations": []}],
            }
        ],
        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
    }


def chat_response(body):
    row = marker(body, "NESSA_HOOK_ROW") or "events-plain"
    phase = marker(body, "NESSA_PHASE")
    if has_tool_result(body) or phase == "second":
        text = "RESUME_OK" if phase == "second" else "TURN_DONE"
        return chat_message(text, "stop")
    names = []
    tool_names(body.get("tools", []), names)
    chosen = pick_tool(names, row)
    if not chosen:
        text = "RESUME_OK" if phase == "second" else "PLAIN_OK"
        return chat_message(text, "stop")
    return {
        "id": f"chat_{Handler.counter}",
        "object": "chat.completion",
        "model": "scripted",
        "choices": [
            {
                "index": 0,
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        {
                            "id": f"call_{Handler.counter}",
                            "type": "function",
                            "function": {
                                "name": chosen,
                                "arguments": tool_arguments(body),
                            },
                        }
                    ],
                },
            }
        ],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
    }


def chat_stream(payload):
    choice = payload["choices"][0]
    message = choice["message"]
    base = {
        "id": payload["id"],
        "object": "chat.completion.chunk",
        "model": payload["model"],
    }
    chunks = [
        {
            **base,
            "choices": [
                {"index": 0, "delta": {"role": "assistant"}, "finish_reason": None}
            ],
        }
    ]
    if message.get("tool_calls"):
        call = message["tool_calls"][0]
        chunks.append(
            {
                **base,
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "tool_calls": [
                                {
                                    "index": 0,
                                    "id": call["id"],
                                    "type": "function",
                                    "function": {
                                        "name": call["function"]["name"],
                                        "arguments": "",
                                    },
                                }
                            ]
                        },
                        "finish_reason": None,
                    }
                ],
            }
        )
        chunks.append(
            {
                **base,
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "tool_calls": [
                                {
                                    "index": 0,
                                    "function": {"arguments": call["function"]["arguments"]},
                                }
                            ]
                        },
                        "finish_reason": None,
                    }
                ],
            }
        )
    elif message.get("content"):
        chunks.append(
            {
                **base,
                "choices": [
                    {
                        "index": 0,
                        "delta": {"content": message["content"]},
                        "finish_reason": None,
                    }
                ],
            }
        )
    chunks.append(
        {
            **base,
            "choices": [{"index": 0, "delta": {}, "finish_reason": choice["finish_reason"]}],
        }
    )
    return chunks


def chat_message(text, finish):
    return {
        "id": f"chat_{Handler.counter}",
        "object": "chat.completion",
        "model": "scripted",
        "choices": [
            {
                "index": 0,
                "finish_reason": finish,
                "message": {"role": "assistant", "content": text},
            }
        ],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
    }


def main():
    if len(sys.argv) != 2:
        print("usage: scripted_openai.py REQUEST_DIR", file=sys.stderr)
        return 2
    request_dir = Path(sys.argv[1])
    request_dir.mkdir(parents=True, exist_ok=True)
    Handler.request_dir = request_dir
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(server.server_address[1], flush=True)
    server.serve_forever()


if __name__ == "__main__":
    sys.exit(main())
