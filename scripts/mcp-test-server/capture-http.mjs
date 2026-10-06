#!/usr/bin/env node
/** Refresh the deterministic local HTTP wire corpus; no credential or model. */
import { createHash } from "node:crypto"
import { readFileSync, writeFileSync } from "node:fs"
import { serveHttp } from "./http-server.mjs"

const out = process.argv[2]
if (!out) {
  process.stderr.write("capture-http.mjs <output.json>\n")
  process.exit(2)
}
const requests = [
  {
    method: "initialize",
    params: {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "contract-probe", version: "1" },
    },
  },
  { method: "ping" },
  { method: "tools/call", params: { name: "show_chart", arguments: {} } },
  { method: "tools/call", params: { name: "always_fails", arguments: {} } },
  { method: "resources/read", params: { uri: "ui://nessa-test/chart.html" } },
  { method: "tools/call", params: { name: "constructor", arguments: {} } },
]
const hash = (path) =>
  createHash("sha256")
    .update(readFileSync(new URL(path, import.meta.url)))
    .digest("hex")
const server = await serveHttp()
try {
  const url = `http://127.0.0.1:${server.address().port}/mcp`
  let session
  const frames = []
  for (const [at, request] of requests.entries()) {
    const frame = { jsonrpc: "2.0", id: at + 1, ...request }
    const response = await fetch(url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        ...(session ? { "mcp-session-id": session } : {}),
      },
      body: JSON.stringify(frame),
      signal: AbortSignal.timeout(5000),
    })
    if (response.status !== 200) throw new Error("HTTP fixture capture refused")
    session ??= response.headers.get("mcp-session-id")
    if (!session) throw new Error("HTTP fixture session missing")
    frames.push({
      request: frame,
      status: response.status,
      response: await response.json(),
    })
  }
  writeFileSync(
    out,
    `${JSON.stringify({ source: { kind: "local-developer-http-server", capturedAt: new Date().toISOString(), recoveredFrom: "b93383c3d", protocolVersion: "2025-06-18", transport: "streamable-http-json", sessionIds: "omitted", serverSha256: hash("./server.mjs"), transportSha256: hash("./http-server.mjs"), probeSha256: hash("./capture-http.mjs") }, frames }, null, 2)}\n`,
  )
  process.stdout.write(
    `${JSON.stringify({ kind: "completed", frames: frames.length })}\n`,
  )
} finally {
  server.closeAllConnections()
  await new Promise((resolve) => server.close(resolve))
}
