#!/usr/bin/env node
/** Developer HTTP fixture, adapted from 392-remote-mcp (b93383c3d).
 * Same answers as server.mjs; no product transport or OAuth implementation.
 * Lifecycle rows and replay limits are in README.md's HTTP fixture section.
 */
import { randomUUID } from "node:crypto"
import { createServer } from "node:http"
import { fileURLToPath } from "node:url"
import { answer } from "./server.mjs"

export const HTTP_PATH = "/mcp"
export const MAX_BODY_BYTES = 1024 * 1024
const failure = (code, message) => ({
  jsonrpc: "2.0",
  id: null,
  error: { code, message },
})
const json = (response, status, value, headers = {}) => {
  response.writeHead(status, { "content-type": "application/json", ...headers })
  response.end(JSON.stringify(value))
}

/** Drain oversized bodies without retaining them, returning a bounded refusal. */
async function body(request) {
  const chunks = []
  let size = 0
  for await (const chunk of request) {
    size += chunk.length
    if (size <= MAX_BODY_BYTES) chunks.push(chunk)
  }
  if (size > MAX_BODY_BYTES) return { error: "too_large" }
  let value
  try {
    value = JSON.parse(Buffer.concat(chunks).toString("utf8"))
  } catch {
    return { error: "parse" }
  }
  if (
    value === null ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    value.jsonrpc !== "2.0"
  )
    return { error: "invalid" }
  return { value }
}

/**
 * Streamable HTTP defaults to JSON responses; `sse` selects legacy HTTP+SSE.
 * `bearerToken` is a developer fixture secret, not OAuth. Session expiry is
 * checked on requests with `now`; `sessionTtlMs` is an absolute lifetime.
 * The clock seam permits expiry checks without sleeping. Bind loopback only.
 */
export function serveHttp({
  port = 0,
  sse = false,
  bearerToken,
  sessionTtlMs = 60_000,
  now = Date.now,
} = {}) {
  const sessions = new Map()
  const release = (id) => {
    const session = sessions.get(id)
    sessions.delete(id)
    session?.stream?.end()
  }
  const server = createServer(async (request, response) => {
    try {
      if (
        bearerToken !== undefined &&
        request.headers.authorization !== `Bearer ${bearerToken}`
      )
        return json(response, 401, failure(-32001, "Unauthorized"), {
          "www-authenticate": "Bearer",
        })
      for (const [id, session] of sessions) if (now() >= session.expiresAt) release(id)
      const url = new URL(request.url, "http://localhost")
      if (sse && url.pathname === HTTP_PATH && request.method === "GET") {
        const id = randomUUID()
        response.writeHead(200, {
          "content-type": "text/event-stream",
          "cache-control": "no-cache",
        })
        sessions.set(id, { stream: response, expiresAt: now() + sessionTtlMs })
        response.on("close", () => sessions.delete(id))
        response.write(`event: endpoint\ndata: /messages?sessionId=${id}\n\n`)
        return
      }
      const legacyPost = sse && url.pathname === "/messages" && request.method === "POST"
      if (url.pathname !== HTTP_PATH && !legacyPost)
        return json(response, 404, failure(-32601, "Not found"))
      if (!legacyPost && (sse || request.method === "GET"))
        return response.writeHead(405).end()
      const id = legacyPost
        ? url.searchParams.get("sessionId")
        : request.headers["mcp-session-id"]
      if (request.method === "DELETE") {
        if (!sessions.has(id)) return response.writeHead(404).end()
        release(id)
        return response.writeHead(200).end()
      }
      if (request.method !== "POST") return response.writeHead(405).end()
      const parsed = await body(request)
      if (parsed.error) {
        const errors = {
          too_large: [413, -32600, "Request body too large"],
          parse: [400, -32700, "Parse error"],
          invalid: [400, -32600, "Invalid request"],
        }
        const [status, code, message] = errors[parsed.error]
        return json(response, status, failure(code, message))
      }
      const message = parsed.value
      if (!legacyPost && message.method === "initialize") {
        const sessionId = randomUUID()
        sessions.set(sessionId, { expiresAt: now() + sessionTtlMs })
        return json(response, 200, answer(message), { "mcp-session-id": sessionId })
      }
      if (typeof id !== "string")
        return json(response, 400, failure(-32600, "No session"))
      const session = sessions.get(id)
      if (!session) return json(response, 404, failure(-32600, "Unknown session"))
      const reply = answer(message)
      if (legacyPost) {
        response.writeHead(202).end()
        if (reply)
          session.stream.write(`event: message\ndata: ${JSON.stringify(reply)}\n\n`)
      } else if (reply) json(response, 200, reply)
      else response.writeHead(202).end()
    } catch {
      if (!response.headersSent)
        json(response, 400, failure(-32600, "Request interrupted"))
      else response.destroy()
    }
  })
  server.on("close", () => {
    for (const id of sessions.keys()) release(id)
  })
  return new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(port, "127.0.0.1", () => resolve(server))
  })
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const port = Number(process.argv[2] ?? 0)
  if (!Number.isInteger(port) || port < 0 || port > 65535) {
    process.stderr.write("http-server.mjs needs a port (0–65535)\n")
    process.exitCode = 2
  } else {
    const server = await serveHttp({ port, sse: process.argv.includes("--sse") })
    process.stderr.write(
      `MCP fixture listening on 127.0.0.1:${server.address().port}/mcp\n`,
    )
  }
}
