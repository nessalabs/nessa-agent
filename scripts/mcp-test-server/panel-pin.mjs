#!/usr/bin/env node
/**
 * A second MCP App for the stress check's window step (`panel_pin`).
 * It is its own server, `panelpin`, so its widget is a second tab beside
 * `mcptest`'s talk board. Its button asks to be shown fullscreen, which the
 * desktop opens as a tab in the chat's panel.
 *
 *   node scripts/mcp-test-server/panel-pin.mjs
 */
import { createInterface } from "node:readline"
import { fileURLToPath } from "node:url"

export const PROTOCOL_VERSION = "2025-06-18"
export const PIN_URI = "ui://panel-pin/pin.html"
export const APP_MIME_TYPE = "text/html;profile=mcp-app"
export const TOOL_NAME = "panel_pin"

const PIN_HTML = `<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Panel pin</title></head>
<body data-pin-state="loading">
<button data-pin="panel">Open in panel</button>
<script>
(function () {
  var parentWindow = window.parent;
  var body = document.body;
  var next = 1;
  var waiting = {};
  function post(message) { parentWindow.postMessage(message, "*"); }
  function ask(method, params) {
    var id = next++;
    post({ jsonrpc: "2.0", id: id, method: method, params: params });
    return new Promise(function (resolve) { waiting[id] = resolve; });
  }
  function tell(method, params) { post({ jsonrpc: "2.0", method: method, params: params }); }
  window.addEventListener("message", function (event) {
    if (event.source !== parentWindow) return;
    var message = event.data;
    if (!message || message.jsonrpc !== "2.0") return;
    if (message.id !== undefined && !message.method) {
      if (!Object.prototype.hasOwnProperty.call(waiting, message.id)) return;
      var resolve = waiting[message.id];
      delete waiting[message.id];
      resolve(message);
    }
  });
  ask("ui/initialize", {
    appInfo: { name: "Panel pin", version: "1.0.0" },
    appCapabilities: { availableDisplayModes: ["inline", "fullscreen"] },
    protocolVersion: "2026-01-26"
  }).then(function (answer) {
    if (answer.error) { body.setAttribute("data-pin-state", "refused"); return; }
    body.setAttribute("data-pin-state", "live");
    tell("ui/notifications/initialized", {});
  });
  document.querySelector('[data-pin="panel"]').addEventListener("click", function () {
    ask("ui/request-display-mode", { mode: "fullscreen" });
  });
})();
</script>
</body>
</html>
`

const failure = (id, code, message) => ({
  jsonrpc: "2.0",
  id,
  error: { code, message },
})

/** One JSON-RPC answer, or null for a notification. */
export function answer(message) {
  if (!message || typeof message !== "object" || message.jsonrpc !== "2.0")
    return failure(message?.id ?? null, -32600, "Invalid request")
  if (!Object.hasOwn(message, "id") || !Object.hasOwn(message, "method")) return null
  const { id, method, params = {} } = message
  const ok = (result) => ({ jsonrpc: "2.0", id, result })
  switch (method) {
    case "initialize":
      return ok({
        protocolVersion: PROTOCOL_VERSION,
        capabilities: { tools: {}, resources: {} },
        serverInfo: { name: "panel-pin", version: "1.0.0" },
      })
    case "ping":
      return ok({})
    case "tools/list":
      return ok({
        tools: [
          {
            name: TOOL_NAME,
            description:
              "Pin a note in the chat's panel. Its UI is an MCP App. Takes no arguments.",
            inputSchema: { type: "object", properties: {}, additionalProperties: false },
            _meta: { ui: { resourceUri: PIN_URI } },
          },
        ],
      })
    case "tools/call":
      if (params?.name !== TOOL_NAME)
        return failure(id, -32602, `Unknown tool: ${String(params?.name)}`)
      return ok({ content: [{ type: "text", text: "Pinned." }] })
    case "resources/list":
      return ok({ resources: [{ uri: PIN_URI, name: "pin", mimeType: APP_MIME_TYPE }] })
    case "resources/read":
      if (params?.uri !== PIN_URI)
        return failure(id, -32002, `Resource not found: ${String(params?.uri)}`)
      return ok({
        contents: [{ uri: PIN_URI, mimeType: APP_MIME_TYPE, text: PIN_HTML }],
      })
    default:
      return failure(id, -32601, `Method not found: ${String(method)}`)
  }
}

export function serve(input = process.stdin, output = process.stdout) {
  let written = Promise.resolve()
  const write = (reply) => {
    written = written.then(() => output.write(`${JSON.stringify(reply)}\n`))
  }
  const lines = createInterface({ input, crlfDelay: Infinity })
  lines.on("line", (line) => {
    if (!line.trim()) return
    let message
    try {
      message = JSON.parse(line)
    } catch {
      write(failure(null, -32700, "Parse error"))
      return
    }
    const reply = answer(message)
    if (reply) write(reply)
  })
  return lines
}

if (process.argv[1] === fileURLToPath(import.meta.url)) serve()
