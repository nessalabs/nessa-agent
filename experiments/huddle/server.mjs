#!/usr/bin/env node
/**
 * Huddle spike: a stdio MCP server whose MCP App is a voice call with a model
 * around files. Throwaway code to answer two questions inside Nessa's app
 * sandbox on the Mac: does the microphone reach an app, and does a WebRTC
 * call to OpenAI's realtime API connect from it. See README.md beside it.
 *
 *   OPENAI_API_KEY=… HUDDLE_ROOT=/path/to/repo node experiments/huddle/server.mjs
 *
 * No dependencies. The real Huddle server lives in its own repo and reaches
 * the gateway as a client; this one only reads files under HUDDLE_ROOT.
 */
import { appendFileSync, readFileSync, readdirSync, renameSync, rmSync, statSync, realpathSync } from "node:fs"
import { createHash } from "node:crypto"
import { tmpdir } from "node:os"
import { createInterface } from "node:readline"
import { dirname, join, relative, resolve, sep } from "node:path"
import { fileURLToPath } from "node:url"

const PROTOCOL_VERSION = "2025-06-18"
const APP_URI = "ui://huddle/app.html"
const APP_MIME_TYPE = "text/html;profile=mcp-app"
const OPENAI = "https://api.openai.com"
const MODEL = process.env.HUDDLE_MODEL || "gpt-realtime"
const VOICE = process.env.HUDDLE_VOICE || "marin"
const ROOT = realpathSync(resolve(process.env.HUDDLE_ROOT || process.cwd()))
const MAX_FILE_BYTES = 256 * 1024
const MAX_ENTRIES = 400

const APP_HTML = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "app.html"), "utf8")

const INSTRUCTIONS = [
  "You are Huddle, a voice companion inside Nessa, an app for working with coding agents.",
  "Talk like a calm colleague: short sentences, no lists read aloud, no filler.",
  "You can put files on the stage with show, mark them with draw, move with view, and remove marks with clear.",
  "Before you talk about a file, show it. While you explain a part, draw on it: highlight the lines, circle a block, underline a phrase's line.",
  "Use list_files to find files when you do not know a path. Paths are relative to the project root.",
  "If the person interrupts you, stop and listen.",
  "For anything past what is on the stage (reading more code, searching, running or changing things), hand it to Claude with ask_agent:",
  "say in a sentence that you asked, then keep the conversation going. The person approves the request in the chat first.",
  "When a report from Claude arrives, tell the person what it found in a sentence or two. Never go silent while you wait.",
].join(" ")

const object = (properties, required = []) => ({
  type: "object",
  properties,
  required,
  additionalProperties: false,
})

/** A path under ROOT, or an error message. Symlinks out of ROOT are refused. */
function inRoot(path) {
  if (typeof path !== "string" || path === "") return { error: "No path given." }
  let full
  try {
    full = realpathSync(resolve(ROOT, path))
  } catch {
    return { error: `No such file: ${path}` }
  }
  if (full !== ROOT && !full.startsWith(ROOT + sep)) return { error: "That path is outside the project." }
  return { full, rel: relative(ROOT, full) || "." }
}

const text = (value) => ({ content: [{ type: "text", text: value }] })
const failed = (value) => ({ content: [{ type: "text", text: value }], isError: true })

/**
 * The key's header, or none when a proxy adds the key on the way out
 * (HUDDLE_KEY_FROM_PROXY=1, as a cloud sandbox's network secret does), or
 * `undefined` when there is no key at all.
 */
function keyHeaders() {
  if (process.env.HUDDLE_KEY_FROM_PROXY === "1") return {}
  const key = process.env.OPENAI_API_KEY
  return key ? { Authorization: `Bearer ${key}` } : undefined
}

const NO_KEY = "OPENAI_API_KEY is not set for the Huddle server."

async function startCall() {
  const auth = keyHeaders()
  if (!auth) return failed(NO_KEY)
  const response = await fetch(`${OPENAI}/v1/realtime/client_secrets`, {
    method: "POST",
    headers: { ...auth, "Content-Type": "application/json" },
    body: JSON.stringify({
      expires_after: { anchor: "created_at", seconds: 60 },
      session: { type: "realtime", model: MODEL, instructions: INSTRUCTIONS, audio: { output: { voice: VOICE } } },
    }),
  })
  const body = await response.text()
  if (!response.ok) return failed(`OpenAI refused the key request (${response.status}): ${body.slice(0, 400)}`)
  const { value, expires_at: expiresAt } = JSON.parse(body)
  return { ...text("A short-lived key for one call."), structuredContent: { clientSecret: value, expiresAt, model: MODEL } }
}

/**
 * The fallback for a page whose own request to OpenAI is refused (its origin
 * is opaque): this server sends the call's offer with the real key, and the
 * page gets the answer. The audio still flows between the page and OpenAI.
 */
async function connect({ sdp }) {
  const auth = keyHeaders()
  if (!auth) return failed(NO_KEY)
  const form = new FormData()
  form.set("sdp", sdp)
  form.set(
    "session",
    JSON.stringify({ type: "realtime", model: MODEL, instructions: INSTRUCTIONS, audio: { output: { voice: VOICE } } }),
  )
  const response = await fetch(`${OPENAI}/v1/realtime/calls`, {
    method: "POST",
    headers: auth,
    body: form,
  })
  const answer = await response.text()
  if (!response.ok) return failed(`OpenAI refused the call (${response.status}): ${answer.slice(0, 400)}`)
  return { ...text("The call's answer."), structuredContent: { sdp: answer } }
}

function readFile({ path }) {
  const found = inRoot(path)
  if (found.error) return failed(found.error)
  const stat = statSync(found.full)
  if (!stat.isFile()) return failed(`${found.rel} is not a file.`)
  if (stat.size > MAX_FILE_BYTES) return failed(`${found.rel} is larger than ${MAX_FILE_BYTES} bytes.`)
  const content = readFileSync(found.full, "utf8")
  return { ...text(`${found.rel}, ${content.split("\n").length} lines.`), structuredContent: { path: found.rel, content } }
}

const SKIP = new Set([".git", "node_modules", "target", "dist", ".vendor"])

function listFiles({ dir = "." }) {
  const found = inRoot(dir)
  if (found.error) return failed(found.error)
  const entries = []
  const walk = (at, depth) => {
    for (const name of readdirSync(at).sort()) {
      if (entries.length >= MAX_ENTRIES || SKIP.has(name) || name.startsWith(".")) continue
      const full = join(at, name)
      const isDir = statSync(full).isDirectory()
      entries.push(relative(ROOT, full) + (isDir ? "/" : ""))
      if (isDir && depth < 2) walk(full, depth + 1)
    }
  }
  walk(found.full, 0)
  return { ...text(entries.join("\n")), structuredContent: { entries } }
}

// Reports from the agent a Huddle handed a task to, held until the app collects them.
// A file, not memory: the agent's copy of this server and the app's are separate
// processes, started by different hosts with the same environment.
const INBOX = join(tmpdir(), `huddle-inbox-${createHash("sha256").update(ROOT).digest("hex").slice(0, 16)}.jsonl`)
const MAX_REPORTS = 20

function report({ summary, agents }) {
  if (typeof summary !== "string" || !summary.trim()) return failed("Give a summary.")
  const started = (Array.isArray(agents) ? agents : [])
    .filter((agent) => agent && typeof agent.title === "string" && typeof agent.sessionId === "string" && agent.sessionId)
    .slice(0, 8)
    .map((agent) => ({ title: agent.title.slice(0, 120), sessionId: agent.sessionId.slice(0, 200) }))
  appendFileSync(INBOX, JSON.stringify({ summary: summary.trim().slice(0, 2000), agents: started, at: Date.now() }) + "\n", { mode: 0o600 })
  return text("Reported to the Huddle.")
}

function collect() {
  // Taken by renaming, so a report appended meanwhile lands in a new file.
  const taken = `${INBOX}.${process.pid}.${Date.now()}`
  try {
    renameSync(INBOX, taken)
  } catch {
    return { ...text("0 report(s)."), structuredContent: { reports: [] } }
  }
  const reports = readFileSync(taken, "utf8")
    .split("\n")
    .filter(Boolean)
    .flatMap((line) => {
      try {
        return [JSON.parse(line)]
      } catch {
        return []
      }
    })
    .slice(-MAX_REPORTS)
  rmSync(taken, { force: true })
  return { ...text(`${reports.length} report(s).`), structuredContent: { reports } }
}

const TOOLS = {
  huddle_open: {
    description:
      "Open a Huddle: a voice call where you talk with a model that shows files and draws on them. Optionally name a file (relative to the project) to start on.",
    inputSchema: object({ path: { type: "string" } }),
    annotations: { readOnlyHint: true },
    _meta: { ui: { resourceUri: APP_URI } },
    call: ({ path }) => ({
      ...text("Huddle is open."),
      // openedAt lets the app tell a fresh open from an old card drawn again.
      structuredContent: { root: ROOT, path: typeof path === "string" ? path : null, openedAt: Date.now() },
    }),
  },
  huddle_start_call: {
    description: "Mint a short-lived OpenAI realtime key for one call. For the Huddle app only.",
    inputSchema: object({}),
    annotations: { readOnlyHint: false, destructiveHint: false },
    _meta: { ui: { visibility: ["app"] } },
    call: startCall,
  },
  huddle_connect: {
    description: "Send a call offer to OpenAI and return its answer. For the Huddle app only.",
    inputSchema: object({ sdp: { type: "string" } }, ["sdp"]),
    annotations: { readOnlyHint: false, destructiveHint: false },
    _meta: { ui: { visibility: ["app"] } },
    call: connect,
  },
  huddle_read_file: {
    description: "Read one file under the project root. For the Huddle app only.",
    inputSchema: object({ path: { type: "string" } }, ["path"]),
    annotations: { readOnlyHint: true },
    _meta: { ui: { visibility: ["app"] } },
    call: readFile,
  },
  huddle_report: {
    description:
      "Report back to the open Huddle after finishing a task it handed you: a 1-3 sentence summary, and the title and session id of any agent threads you started.",
    inputSchema: object(
      {
        summary: { type: "string" },
        agents: {
          type: "array",
          items: object({ title: { type: "string" }, sessionId: { type: "string" } }, ["title", "sessionId"]),
        },
      },
      ["summary"],
    ),
    annotations: { readOnlyHint: false, destructiveHint: false },
    call: report,
  },
  huddle_inbox: {
    description: "Collect and clear the reports agents sent this Huddle. For the Huddle app only.",
    inputSchema: object({}),
    annotations: { readOnlyHint: false, destructiveHint: false },
    _meta: { ui: { visibility: ["app"] } },
    call: collect,
  },
  huddle_list_files: {
    description: "List files under a folder of the project, two levels deep. For the Huddle app only.",
    inputSchema: object({ dir: { type: "string" } }),
    annotations: { readOnlyHint: true },
    _meta: { ui: { visibility: ["app"] } },
    call: listFiles,
  },
}

const RESOURCES = {
  [APP_URI]: {
    name: "huddle",
    mimeType: APP_MIME_TYPE,
    text: APP_HTML,
    _meta: {
      ui: {
        csp: { connectDomains: [OPENAI], resourceDomains: ["https://cdn.jsdelivr.net"] },
        permissions: { microphone: {} },
        prefersBorder: false,
      },
    },
  },
}

const failure = (id, code, message) => ({ jsonrpc: "2.0", id, error: { code, message } })

async function answer(message) {
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
        serverInfo: { name: "huddle", version: "0.0.1-spike" },
      })
    case "ping":
      return ok({})
    case "tools/list":
      return ok({ tools: Object.entries(TOOLS).map(([name, { call, ...declared }]) => ({ name, ...declared })) })
    case "tools/call": {
      const tool = TOOLS[params?.name]
      if (!tool) return failure(id, -32602, `Unknown tool: ${String(params?.name)}`)
      try {
        return ok(await tool.call(params.arguments ?? {}))
      } catch (error) {
        return ok(failed(`${params.name} failed: ${error instanceof Error ? error.message : String(error)}`))
      }
    }
    case "resources/list":
      return ok({ resources: Object.entries(RESOURCES).map(([uri, { name, mimeType }]) => ({ uri, name, mimeType })) })
    case "resources/read": {
      const resource = RESOURCES[params?.uri]
      if (!resource) return failure(id, -32002, `Resource not found: ${String(params?.uri)}`)
      const { mimeType, text: body, _meta } = resource
      return ok({ contents: [{ uri: params.uri, mimeType, text: body, _meta }] })
    }
    default:
      return failure(id, -32601, `Method not found: ${String(method)}`)
  }
}

const lines = createInterface({ input: process.stdin, crlfDelay: Infinity })
lines.on("line", async (line) => {
  if (!line.trim()) return
  let message
  try {
    message = JSON.parse(line)
  } catch {
    process.stdout.write(`${JSON.stringify(failure(null, -32700, "Parse error"))}\n`)
    return
  }
  const reply = await answer(message)
  if (reply) process.stdout.write(`${JSON.stringify(reply)}\n`)
})
