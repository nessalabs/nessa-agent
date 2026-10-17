import assert from "node:assert/strict"
import { EventEmitter } from "node:events"
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, resolve } from "node:path"
import { spawnSync } from "node:child_process"
import test from "node:test"
import { facts, observe } from "./matched-request-observer.mjs"

test("request facts retain only fixed classifications and body numbers", () => {
  const frame = {}
  const page = {
    url: () => "http://localhost:1000/desktop.html",
    mainFrame: () => frame,
    isClosed: () => false,
  }
  const request = {
    url: () => "http://127.0.0.1:2000/mcp-resources?token=PRIVATE_MARKER",
    existingResponse: () => ({
      status: () => 200,
      headers: () => ({ "content-length": "123", cookie: "PRIVATE_MARKER" }),
    }),
    frame: () => frame,
    method: () => "GET",
    resourceType: () => "fetch",
    failure: () => ({ errorText: "net::ERR_ABORTED" }),
  }
  const result = facts(request, page, true, "chromium")
  assert.equal(result.endpoint, "/mcp-resources")
  assert.equal(result.sameOrigin, false)
  assert.equal(result.duringClose, true)
  assert.equal(result.status, 200)
  assert.equal(result.contentLength, 123)
  assert.equal(result.aborted, true)
  assert.ok(!JSON.stringify(result).includes("PRIVATE_MARKER"))
  assert.ok(!JSON.stringify(result).includes("http"))
})

test("observation preserves launch, context and close arguments and outcomes", async () => {
  const context = new EventEmitter()
  const closeError = new Error("original close")
  context.close = function (...args) {
    assert.equal(this, context)
    assert.deepEqual(args, ["close-arg"])
    throw closeError
  }
  const browser = {
    newContext: async function (...args) {
      assert.equal(this, browser)
      assert.deepEqual(args, ["context-arg"])
      return context
    },
  }
  const type = {
    launch: async function (...args) {
      assert.equal(this, type)
      assert.deepEqual(args, ["launch-arg"])
      return browser
    },
  }
  const records = []
  observe(type, "chromium", records)
  assert.equal(await type.launch("launch-arg"), browser)
  assert.equal(await browser.newContext("context-arg"), context)
  const page = new EventEmitter()
  Object.assign(page, {
    url: () => "http://localhost/desktop.html",
    mainFrame: () => null,
    isClosed: () => false,
  })
  context.emit("page", page)
  const request = {
    url: () => "http://localhost/browser/check",
    existingResponse: () => ({ status: () => 204, headers: () => ({}) }),
    frame: () => null,
    method: () => "GET",
    resourceType: () => "fetch",
    failure: () => ({ errorText: "net::ERR_ABORTED" }),
    sizes: () => Promise.resolve({ responseBodySize: 0 }),
  }
  page.emit("requestfailed", request)
  assert.equal(records[0].duringClose, false)
  assert.throws(
    () => context.close("close-arg"),
    (error) => error === closeError,
  )
  page.emit("requestfailed", request)
  await Promise.resolve()
  assert.equal(records[1].duringClose, true)
  assert.equal(records[0].bodySize, 0)
  for (let i = 0; i < 100; i++)
    page.emit("requestfailed", {
      ...request,
      sizes: () => {
        throw Error("sizes unavailable")
      },
    })
  assert.equal(records.length, 64)
})

test("actual preload runs only in gateway-window and preserves its exit status", () => {
  const directory = mkdtempSync(join(tmpdir(), "matched-preload-"))
  try {
    mkdirSync(join(directory, "node_modules/playwright"), { recursive: true })
    writeFileSync(join(directory, "package.json"), "{}")
    writeFileSync(
      join(directory, "node_modules/playwright/index.js"),
      "module.exports={chromium:{launch(){}},webkit:{launch(){}}}",
    )
    writeFileSync(join(directory, "gateway-window.mjs"), "process.exit(7)")
    writeFileSync(join(directory, "other.mjs"), "process.exit(7)")
    const output = join(directory, "facts.json")
    const preload = resolve(import.meta.dirname, "matched-request-observer.mjs")
    const retained = join(directory, "retained observer.mjs")
    writeFileSync(retained, readFileSync(preload))
    const env = {
      ...process.env,
      MATCHED_REQUEST_OUTPUT: output,
      NODE_OPTIONS: `--import="${retained}"`,
    }
    const run = (name) => spawnSync(process.execPath, [name], { cwd: directory, env })
    assert.equal(run("gateway-window.mjs").status, 7)
    assert.deepEqual(JSON.parse(readFileSync(output)), { setup: true, requests: [] })
    rmSync(output)
    assert.equal(run("other.mjs").status, 7)
    assert.throws(() => readFileSync(output), { code: "ENOENT" })
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})
