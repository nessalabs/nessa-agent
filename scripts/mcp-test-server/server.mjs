#!/usr/bin/env node
/**
 * A stdio MCP server for testing what reaches Nessa from an MCP tool call:
 * structured results, resource links and embedded resources, a dotted tool
 * name, a tool error, and MCP Apps tools with `ui://` resources — one of them
 * an app that calls, through its host, a destructive tool only apps may call
 * and tools hidden from apps (#384). It has no side effects and no
 * dependencies; every answer is fixed, so a recording of one run can be
 * compared with another.
 *
 *   node scripts/mcp-test-server/server.mjs
 *
 * See README.md beside it for what each tool is for and how the live check
 * (`scripts/mcp-test-server/live-check.mjs`) uses it.
 */
import { createInterface } from "node:readline"
import { fileURLToPath } from "node:url"

/** The protocol revision this server speaks: the one with structured results. */
export const PROTOCOL_VERSION = "2025-06-18"

/** The MCP Apps resource the `show_chart` tool declares as its UI. */
export const CHART_URI = "ui://nessa-test/chart.html"
/** The MIME type MCP Apps gives an app's HTML. */
export const APP_MIME_TYPE = "text/html;profile=mcp-app"

/** The MCP Apps resource the `review_rows` tool declares as its UI. */
export const REVIEW_URI = "ui://nessa-test/review.html"

/**
 * The tools the review app calls through its host's `tools/call`, by what
 * each is for: a destructive tool only an app may call, and two tools hidden
 * from apps — one declaring no UI (`resourceUri` is optional in `_meta.ui`),
 * one declaring the chart's.
 */
export const APP_CALLS = {
  destructive: "app_delete_row",
  hiddenNoUi: "model_only_note",
  hiddenWithUi: "model_only_chart",
}

const CHART_HTML =
  "<!doctype html><html><body><p id=chart>chart for nessa-test</p></body></html>"

/**
 * The review app: speaks the MCP Apps `ui/*` bridge by hand (2026-01-26), as
 * the spec shows an app can without an SDK. Shown inline, once it has its
 * tool result it calls, through the host's `tools/call`, the destructive tool
 * (`#first`) and both hidden tools (`#hidden-no-ui`, `#hidden-with-ui`); its
 * `delete` button calls the destructive tool again (`#again`), and its
 * `fullscreen` button asks to be shown fullscreen. Each output says
 * `pending` until it is answered, then `ok: <the result's text>` or
 * `error: <the error's message>`. Its state and display mode are on its
 * body (`data-review-state`, `data-review-mode`), for a browser to read.
 */
const REVIEW_HTML = `<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Review rows</title>
<style>body { margin: 0; padding: 12px; font: 13px/1.4 system-ui, sans-serif; } output { display: block; min-height: 1.4em; }</style>
</head>
<body data-review-state="loading">
<h1 style="font-size:14px;margin:0 0 8px">Review rows (nessa-test)</h1>
<button data-review="delete">Delete row 2 again</button>
<button data-review="fullscreen">Fullscreen</button>
<output id="result"></output>
<output id="first"></output>
<output id="hidden-no-ui"></output>
<output id="hidden-with-ui"></output>
<output id="again"></output>
<script>
(function () {
  var parentWindow = window.parent;
  var body = document.body;
  var next = 1;
  var waiting = {};
  var mode = null;
  var called = false;
  function post(message) { parentWindow.postMessage(message, "*"); }
  function ask(method, params) {
    var id = next++;
    post({ jsonrpc: "2.0", id: id, method: method, params: params });
    return new Promise(function (resolve) { waiting[id] = resolve; });
  }
  function tell(method, params) { post({ jsonrpc: "2.0", method: method, params: params }); }
  function show(id, text) { document.getElementById(id).textContent = text; }
  function outcome(message) {
    if (message.error) return "error: " + message.error.message;
    var text = (message.result && message.result.content || [])
      .filter(function (block) { return block.type === "text"; })
      .map(function (block) { return block.text; }).join(" ");
    return (message.result && message.result.isError ? "isError: " : "ok: ") + text;
  }
  function call(output, name, args) {
    show(output, "pending");
    ask("tools/call", { name: name, arguments: args }).then(function (m) { show(output, outcome(m)); });
  }
  window.addEventListener("message", function (event) {
    if (event.source !== parentWindow) return;
    var message = event.data;
    if (!message || message.jsonrpc !== "2.0") return;
    if (message.id !== undefined && !message.method) {
      var resolve = waiting[message.id];
      delete waiting[message.id];
      if (resolve) resolve(message);
      return;
    }
    if (message.method === "ui/notifications/host-context-changed" && message.params.displayMode) {
      mode = message.params.displayMode;
      body.setAttribute("data-review-mode", mode);
    }
    if (message.method === "ui/notifications/tool-result") {
      show("result", JSON.stringify(message.params.structuredContent));
      // Inline, once: the calls this app exists to make.
      if (mode !== "inline" || called) return;
      called = true;
      call("first", ${JSON.stringify(APP_CALLS.destructive)}, { id: 2 });
      call("hidden-no-ui", ${JSON.stringify(APP_CALLS.hiddenNoUi)}, {});
      call("hidden-with-ui", ${JSON.stringify(APP_CALLS.hiddenWithUi)}, {});
    }
    if (message.method === "ui/resource-teardown") post({ jsonrpc: "2.0", id: message.id, result: {} });
  });
  function reportSize() {
    tell("ui/notifications/size-changed", {
      width: document.documentElement.scrollWidth,
      height: document.documentElement.scrollHeight
    });
  }
  ask("ui/initialize", {
    appInfo: { name: "Review rows", version: "1.0.0" },
    appCapabilities: { availableDisplayModes: ["inline", "fullscreen"] },
    protocolVersion: "2026-01-26"
  }).then(function (answer) {
    if (answer.error) { body.setAttribute("data-review-state", "refused"); return; }
    // A host may leave the display mode out; it is then the spec's default, inline.
    mode = answer.result.hostContext && answer.result.hostContext.displayMode || "inline";
    body.setAttribute("data-review-mode", mode);
    body.setAttribute("data-review-state", "live");
    tell("ui/notifications/initialized", {});
    reportSize();
    new ResizeObserver(reportSize).observe(body);
  });
  document.querySelector('[data-review="delete"]').addEventListener("click", function () {
    call("again", ${JSON.stringify(APP_CALLS.destructive)}, { id: 2 });
  });
  document.querySelector('[data-review="fullscreen"]').addEventListener("click", function () {
    ask("ui/request-display-mode", { mode: "fullscreen" });
  });
})();
</script>
</body>
</html>
`

const ROWS = [
  { id: 1, name: "alpha", value: 10 },
  { id: 2, name: "beta", value: 20 },
]

const object = (properties, required = Object.keys(properties)) => ({
  type: "object",
  properties,
  required,
  additionalProperties: false,
})

/**
 * Every tool, with what `tools/list` says of it and what `tools/call` answers.
 * Answers are fixed: the arguments are checked for shape, never echoed, so a
 * model cannot make the result say something the recording did not expect.
 */
export const TOOLS = {
  report_rows: {
    description:
      "Report two rows of test data as text and as a structured result. Takes no arguments.",
    inputSchema: object({}),
    outputSchema: object({ rows: { type: "array" }, total: { type: "number" } }),
    call: () => ({
      content: [{ type: "text", text: "Two rows: alpha (10), beta (20)." }],
      structuredContent: { rows: ROWS, total: 30 },
    }),
  },
  link_resources: {
    description:
      "Return a resource link and an embedded text resource. Takes no arguments.",
    inputSchema: object({}),
    call: () => ({
      content: [
        { type: "text", text: "One link and one embedded resource follow." },
        {
          type: "resource_link",
          uri: "file:///nessa-test/rows.csv",
          name: "rows.csv",
          mimeType: "text/csv",
        },
        {
          type: "resource",
          resource: {
            uri: "file:///nessa-test/notes.txt",
            mimeType: "text/plain",
            text: "Embedded note from nessa-test.",
          },
        },
      ],
    }),
  },
  "rows.get": {
    description:
      "Get one test row by id (1 or 2). The dot in this tool's name is deliberate.",
    inputSchema: object({ id: { type: "integer" } }),
    call: ({ id }) => {
      const row = ROWS.find((each) => each.id === id)
      return row
        ? {
            content: [{ type: "text", text: `Row ${row.id}: ${row.name}` }],
            structuredContent: row,
          }
        : { content: [{ type: "text", text: `No row ${id}.` }], isError: true }
    },
  },
  always_fails: {
    description:
      "A tool that always reports an error result (isError). Takes no arguments.",
    inputSchema: object({}),
    call: () => ({
      content: [{ type: "text", text: "This tool always fails, on purpose." }],
      isError: true,
    }),
  },
  show_chart: {
    description:
      "Show a chart of the test rows. Its UI is an MCP App. Takes no arguments.",
    inputSchema: object({}),
    _meta: { ui: { resourceUri: CHART_URI }, "openai/outputTemplate": CHART_URI },
    call: () => ({
      content: [{ type: "text", text: "Chart of two rows." }],
      structuredContent: { series: ROWS.map(({ name, value }) => ({ name, value })) },
    }),
  },
  review_rows: {
    description:
      "Show the test rows for review. Its UI is an MCP App that calls tools of its own. Takes no arguments.",
    inputSchema: object({}),
    annotations: { readOnlyHint: true },
    _meta: { ui: { resourceUri: REVIEW_URI } },
    call: () => ({
      content: [{ type: "text", text: "Two rows to review." }],
      structuredContent: { rows: ROWS.map(({ id }) => id) },
    }),
  },
  // Only an app may call it, and it says it destroys: a host asks the person
  // first. It deletes nothing; its answer is fixed.
  [APP_CALLS.destructive]: {
    description: "Delete one test row by id (1 or 2). For the review app only.",
    inputSchema: object({ id: { type: "integer" } }),
    annotations: { readOnlyHint: false, destructiveHint: true },
    _meta: { ui: { visibility: ["app"] } },
    call: ({ id }) =>
      ROWS.some((row) => row.id === id)
        ? { content: [{ type: "text", text: `Deleted row ${id}.` }] }
        : { content: [{ type: "text", text: `No row ${id}.` }], isError: true },
  },
  // Hidden from apps, declaring no UI: `visibility` alone.
  [APP_CALLS.hiddenNoUi]: {
    description: "Return a note for the model only. Takes no arguments.",
    inputSchema: object({}),
    annotations: { readOnlyHint: true },
    _meta: { ui: { visibility: ["model"] } },
    call: () => ({ content: [{ type: "text", text: "A note for the model only." }] }),
  },
  // Hidden from apps, declaring a UI of its own.
  [APP_CALLS.hiddenWithUi]: {
    description: "Show the chart, for the model only. Takes no arguments.",
    inputSchema: object({}),
    annotations: { readOnlyHint: true },
    _meta: { ui: { resourceUri: CHART_URI, visibility: ["model"] } },
    call: () => ({ content: [{ type: "text", text: "A chart for the model only." }] }),
  },
}

const RESOURCES = {
  [CHART_URI]: {
    name: "chart",
    mimeType: APP_MIME_TYPE,
    text: CHART_HTML,
    _meta: { ui: { csp: { connectDomains: [], resourceDomains: [] } } },
  },
  [REVIEW_URI]: {
    name: "review",
    mimeType: APP_MIME_TYPE,
    text: REVIEW_HTML,
    _meta: { ui: { csp: { connectDomains: [], resourceDomains: [] } } },
  },
}

function argumentsValid(schema, args) {
  if (args === undefined) args = {}
  if (!args || typeof args !== "object" || Array.isArray(args)) return false
  const keys = Object.keys(args)
  if (keys.some((key) => !Object.hasOwn(schema.properties, key))) return false
  return schema.required.every(
    (key) =>
      Object.hasOwn(args, key) &&
      (schema.properties[key].type !== "integer" || Number.isInteger(args[key])),
  )
}

const failure = (id, code, message) => ({ jsonrpc: "2.0", id, error: { code, message } })

/**
 * The answer to one JSON-RPC message, or `null` for a notification (no `id`).
 * Pure: the stdio loop below is the only thing that reads or writes.
 */
export function answer(message) {
  if (!message || typeof message !== "object" || message.jsonrpc !== "2.0")
    return failure(message?.id ?? null, -32600, "Invalid request")
  // A notification, or a response (this server sends no requests): no answer.
  if (!Object.hasOwn(message, "id") || !Object.hasOwn(message, "method")) return null
  const { id, method, params = {} } = message
  const ok = (result) => ({ jsonrpc: "2.0", id, result })
  switch (method) {
    case "initialize":
      return ok({
        protocolVersion: PROTOCOL_VERSION,
        capabilities: { tools: {}, resources: {} },
        serverInfo: { name: "nessa-test", version: "1.0.0" },
      })
    case "ping":
      return ok({})
    case "tools/list":
      return ok({
        tools: Object.entries(TOOLS).map(([name, { call, ...declared }]) => ({
          name,
          ...declared,
        })),
      })
    case "tools/call": {
      const name = params?.name
      if (typeof name !== "string" || !Object.hasOwn(TOOLS, name))
        return failure(id, -32602, `Unknown tool: ${String(name)}`)
      const tool = TOOLS[name]
      if (!argumentsValid(tool.inputSchema, params.arguments))
        return ok({
          content: [{ type: "text", text: `Invalid arguments for ${name}.` }],
          isError: true,
        })
      return ok(tool.call(params.arguments ?? {}))
    }
    case "resources/list":
      return ok({
        resources: Object.entries(RESOURCES).map(([uri, { name, mimeType }]) => ({
          uri,
          name,
          mimeType,
        })),
      })
    case "resources/read": {
      const uri = params?.uri
      if (typeof uri !== "string" || !Object.hasOwn(RESOURCES, uri))
        return failure(id, -32002, `Resource not found: ${String(uri)}`)
      const { mimeType, text, _meta } = RESOURCES[uri]
      return ok({ contents: [{ uri, mimeType, text, _meta }] })
    }
    default:
      return failure(id, -32601, `Method not found: ${String(method)}`)
  }
}

/** Serve newline-delimited JSON-RPC on `input`, answering on `output`. */
export function serve(input = process.stdin, output = process.stdout) {
  const lines = createInterface({ input, crlfDelay: Infinity })
  lines.on("line", (line) => {
    if (!line.trim()) return
    let message
    try {
      message = JSON.parse(line)
    } catch {
      output.write(`${JSON.stringify(failure(null, -32700, "Parse error"))}\n`)
      return
    }
    const reply = answer(message)
    if (reply) output.write(`${JSON.stringify(reply)}\n`)
  })
  return lines
}

if (process.argv[1] === fileURLToPath(import.meta.url)) serve()
