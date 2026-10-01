#!/usr/bin/env node
/**
 * A stdio MCP server for testing what reaches Nessa from an MCP tool call:
 * structured results, resource links and embedded resources, a dotted tool
 * name, a tool error, and an MCP Apps tool with a `ui://` resource. It has no
 * side effects and no dependencies; every answer is fixed, so a recording of
 * one run can be compared with another.
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

const CHART_HTML =
  "<!doctype html><html><body><p id=chart>chart for nessa-test</p></body></html>"

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
}

const RESOURCES = {
  [CHART_URI]: {
    name: "chart",
    mimeType: APP_MIME_TYPE,
    text: CHART_HTML,
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
  if (!Object.hasOwn(message, "id")) return null
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
