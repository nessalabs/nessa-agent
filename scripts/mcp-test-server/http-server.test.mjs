import { strict as assert } from "node:assert"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import { serveHttp, MAX_BODY_BYTES } from "./http-server.mjs"
import { TOOLS } from "./server.mjs"

const fixture = JSON.parse(
  readFileSync(new URL("./fixtures/http-frames.json", import.meta.url), "utf8"),
)
const rpc = (id, method, params = {}) => ({ jsonrpc: "2.0", id, method, params })
async function listening(t, options) {
  const server = await serveHttp(options)
  t.after(() => {
    server.closeAllConnections()
    server.close()
  })
  return `http://127.0.0.1:${server.address().port}`
}
const post = (base, frame, session, extra = {}) =>
  fetch(`${base}/mcp`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(session ? { "mcp-session-id": session } : {}),
      ...extra,
    },
    body: JSON.stringify(frame),
    signal: AbortSignal.timeout(5000),
  })
async function initialize(base, id = 1, extra) {
  const response = await post(base, rpc(id, "initialize"), undefined, extra)
  assert.equal(response.status, 200)
  await response.json()
  return response.headers.get("mcp-session-id")
}

// Each expected response was recorded from a loopback HTTP session. Comparing
// JSON wire data to that corpus catches representation drift beside status checks.
test("streamable HTTP replays recorded initialize, tool results, errors and UI resource", async (t) => {
  const base = await listening(t)
  let session
  for (const frame of fixture.frames) {
    const response = await post(base, frame.request, session)
    assert.equal(response.status, frame.status)
    session ??= response.headers.get("mcp-session-id")
    assert.ok(session)
    assert.deepEqual(await response.json(), frame.response)
  }
  const list = await post(base, rpc(20, "tools/list"), session)
  assert.deepEqual(
    (await list.json()).result.tools.map(({ name }) => name),
    Object.keys(TOOLS),
  )
})

test("two sessions isolate cleanup; ended and foreign sessions cannot reuse a survivor", async (t) => {
  const base = await listening(t)
  const first = await initialize(base)
  const second = await initialize(base)
  assert.notEqual(first, second)
  assert.equal((await post(base, rpc(2, "ping"))).status, 400)
  assert.equal((await post(base, rpc(2, "ping"), "foreign")).status, 404)
  assert.equal(
    (
      await fetch(`${base}/mcp`, {
        method: "DELETE",
        headers: { "mcp-session-id": first },
      })
    ).status,
    200,
  )
  assert.equal((await post(base, rpc(3, "ping"), first)).status, 404)
  assert.equal((await post(base, rpc(3, "ping"), second)).status, 200)
  assert.equal(
    (
      await fetch(`${base}/mcp`, {
        method: "DELETE",
        headers: { "mcp-session-id": first },
      })
    ).status,
    404,
  )
  assert.equal((await fetch(`${base}/mcp`)).status, 405)
  const notice = await post(
    base,
    { jsonrpc: "2.0", method: "notifications/initialized" },
    second,
  )
  assert.equal(notice.status, 202)
  assert.equal(await notice.text(), "")
})

test("fixture bearer challenge refuses requests before creating or deleting a session", async (t) => {
  const base = await listening(t, { bearerToken: "fixture-token" })
  for (const header of [undefined, "Bearer wrong"]) {
    const response = await post(
      base,
      rpc(1, "initialize"),
      undefined,
      header ? { authorization: header } : {},
    )
    assert.equal(response.status, 401)
    assert.equal(response.headers.get("www-authenticate"), "Bearer")
    assert.equal((await response.json()).error.code, -32001)
  }
  const auth = { authorization: "Bearer fixture-token" }
  const session = await initialize(base, 1, auth)
  assert.equal(
    (
      await fetch(`${base}/mcp`, {
        method: "DELETE",
        headers: { "mcp-session-id": session },
      })
    ).status,
    401,
  )
  assert.equal((await post(base, rpc(2, "ping"), session, auth)).status, 200)
})

test("absolute expiry fences an old session while a newly opened session works", async (t) => {
  let now = 10
  const base = await listening(t, { now: () => now, sessionTtlMs: 50 })
  const first = await initialize(base)
  now = 59
  assert.equal((await post(base, rpc(2, "ping"), first)).status, 200)
  now = 60
  assert.equal((await post(base, rpc(3, "ping"), first)).status, 404)
  const second = await initialize(base)
  assert.notEqual(first, second)
  assert.equal((await post(base, rpc(4, "ping"), second)).status, 200)
})

test("malformed JSON, unsupported batches and oversized bodies refuse without killing the server", async (t) => {
  const base = await listening(t)
  for (const [body, status, code] of [
    ["{", 400, -32700],
    ["null", 400, -32600],
    ["[]", 400, -32600],
    ["{}", 400, -32600],
    ["x".repeat(MAX_BODY_BYTES + 1), 413, -32600],
  ]) {
    const response = await fetch(`${base}/mcp`, {
      method: "POST",
      body,
      signal: AbortSignal.timeout(5000),
    })
    assert.equal(response.status, status)
    assert.equal((await response.json()).error.code, code)
  }
  const session = await initialize(base)
  assert.equal((await post(base, rpc(2, "ping"), session)).status, 200)
})

async function stream(base) {
  const response = await fetch(`${base}/mcp`, { signal: AbortSignal.timeout(5000) })
  assert.equal(response.headers.get("content-type"), "text/event-stream")
  const reader = response.body.getReader()
  let pending = ""
  const decoder = new TextDecoder()
  const next = async () => {
    while (!pending.includes("\n\n")) {
      const { value, done } = await reader.read()
      assert.equal(done, false)
      pending += decoder.decode(value, { stream: true })
    }
    const at = pending.indexOf("\n\n")
    const event = pending.slice(0, at)
    pending = pending.slice(at + 2)
    return event
  }
  const endpoint = (await next()).split("\ndata: ")[1]
  return { reader, endpoint, next }
}
const legacyPost = (base, endpoint, frame) =>
  fetch(new URL(endpoint, base), {
    method: "POST",
    body: JSON.stringify(frame),
    signal: AbortSignal.timeout(5000),
  })

test("legacy SSE delivers the same recorded frames on each isolated stream, then cleans disconnected sessions", async (t) => {
  const base = await listening(t, { sse: true })
  assert.equal((await post(base, rpc(1, "initialize"))).status, 405)
  const first = await stream(base)
  const second = await stream(base)
  t.after(() => {
    first.reader.cancel().catch(() => {})
    second.reader.cancel().catch(() => {})
  })
  assert.notEqual(first.endpoint, second.endpoint)
  for (const frame of fixture.frames) {
    assert.equal((await legacyPost(base, first.endpoint, frame.request)).status, 202)
    const event = await first.next()
    assert.equal(event.split("\n")[0], "event: message")
    assert.deepEqual(JSON.parse(event.split("\ndata: ")[1]), frame.response)
  }
  assert.equal((await legacyPost(base, second.endpoint, rpc(70, "ping"))).status, 202)
  assert.equal(JSON.parse((await second.next()).split("\ndata: ")[1]).id, 70)
  await first.reader.cancel()
  // Cancellation is observed across an actual socket: bounded polling observes
  // removal rather than assuming it happened after an arbitrary sleep.
  const deadline = Date.now() + 2000
  let status
  do {
    status = (await legacyPost(base, first.endpoint, rpc(71, "ping"))).status
  } while (status !== 404 && Date.now() < deadline)
  assert.equal(status, 404)
  assert.equal((await legacyPost(base, second.endpoint, rpc(72, "ping"))).status, 202)
  assert.equal(JSON.parse((await second.next()).split("\ndata: ")[1]).id, 72)
})

test("legacy expiry ends the stream and refuses its message endpoint", async (t) => {
  let now = 0
  const base = await listening(t, { sse: true, now: () => now, sessionTtlMs: 20 })
  const session = await stream(base)
  t.after(() => session.reader.cancel().catch(() => {}))
  now = 20
  assert.equal(
    (await legacyPost(base, session.endpoint, rpc(1, "initialize"))).status,
    404,
  )
  assert.equal((await session.reader.read()).done, true)
})
