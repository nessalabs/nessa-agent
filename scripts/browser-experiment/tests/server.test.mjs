import test from "node:test"
import assert from "node:assert/strict"
import { request } from "node:http"
import { viewerServer } from "../runtime/server.mjs"
import { createSession } from "../runtime/session.mjs"

test("loopback API requires capability, exact origin/host, bounded JSON; no traversal or evaluation", async () => {
  let clicks = 0
  const session = createSession(
    {
      locator: () => ({
        click: async () => {
          clicks++
        },
      }),
      screenshot: async () => Buffer.from("test-frame"),
    },
    "http://127.0.0.1:1234/",
  )
  const server = await viewerServer(session)
  const auth = { Authorization: `Bearer ${server.token}` }
  try {
    assert.match(server.origin, /^http:\/\/127\.0\.0\.1:\d+$/)
    const get = (path, headers = {}) => fetch(server.origin + path, { headers })
    assert.equal((await get("/frame")).status, 401)
    assert.equal((await get("/status", { Authorization: "Bearer wrong" })).status, 401)
    assert.equal(
      (await get("/frame", { ...auth, Origin: "http://evil.invalid" })).status,
      403,
    )
    const reboundStatus = await new Promise((resolve, reject) => {
      const req = request(server.origin, { headers: { Host: "evil.invalid" } }, (res) => {
        res.resume()
        resolve(res.statusCode)
      })
      req.on("error", reject)
      req.end()
    })
    assert.equal(reboundStatus, 403)
    const page = await get("/")
    assert.equal(page.status, 200)
    assert.match(page.headers.get("content-security-policy"), /frame-ancestors 'self'/)
    assert.equal(page.headers.get("cache-control"), "no-store")
    assert.ok(!(await page.text()).includes(server.token))
    assert.equal((await get("/../package.json", auth)).status, 404)
    const post = (body, extra = {}) =>
      fetch(server.origin + "/command", {
        method: "POST",
        headers: { ...auth, "Content-Type": "application/json", ...extra },
        body,
      })
    assert.equal(
      (await post('{"type":"increment"}', { Origin: "http://evil.invalid" })).status,
      403,
    )
    assert.equal((await post("{}", { "Content-Type": "text/plain" })).status, 415)
    assert.equal((await post("{")).status, 409)
    const malformed = await post('{"type":{"toString":null}}')
    assert.equal(malformed.status, 409)
    assert.deepEqual(await malformed.json(), { error: "invalid_command" })
    assert.equal((await post("x".repeat(4097))).status, 413)
    for (const type of ["evaluate", "__proto__", "frame"])
      assert.equal((await post(JSON.stringify({ type }))).status, 409)
    assert.equal(clicks, 0)
    assert.equal((await post('{"type":"increment"}')).status, 200)
    assert.equal(clicks, 1)
    assert.equal(await (await get("/frame", auth)).text(), "test-frame")
    await session.close()
    assert.equal((await post('{"type":"increment"}')).status, 409)
  } finally {
    await session.close()
    await server.close()
  }
})
