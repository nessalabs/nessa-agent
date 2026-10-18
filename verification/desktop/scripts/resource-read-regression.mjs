#!/usr/bin/env node
/** Synthetic HTTP/ports through the real Vite client and bridge, in both engines.
 * This is a harness regression, not an authenticated gateway or iframe check;
 * gateway-window.mjs separately requires real app activation and data flow.
 */
import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { createServer } from "node:http"
import { mkdir } from "node:fs/promises"
import { join } from "node:path"
import {
  launch,
  classifyResourceFailure,
  withCleanup,
  enqueueSizeReport,
  settleSizeReports,
} from "./lib/browser.mjs"
import { installResourceObservation } from "./lib/resource-read-modules.mjs"
import { resourceEndpoint } from "./lib/resource-read-evidence.mjs"
import { captureOpenFailure, watchOpenRequests } from "./lib/open-failure-evidence.mjs"
import { settled } from "./lib/workspace.mjs"
import { main } from "./lib/run.mjs"
import { startDevServer } from "./lib/server.mjs"
import { css } from "./lib/selectors.mjs"

const controls = [
  "complete",
  "corrupt",
  "truncated",
  "remove-during-fetch",
  "remove-during-digest",
  "remove-after-live",
  "unrelated-request",
  "duplicate-request",
  "large-complete",
  "two-mounts",
]

/** Runs in the fixture page, importing the exact decorated production modules. */
async function readFixture(input) {
  const [
    { createAppBridge },
    { fetchMcpResource },
    { createMcpAppsApi },
    { fixtureCall },
    { sandboxMethods },
  ] = await Promise.all([
    import("/src/desktop/widgets/app/application/bridge.ts"),
    import("/packages/nessa-client/src/transport/mcp-resource-fetch.ts"),
    import("/packages/nessa-client/src/presentation/mcp-apps-api.ts"),
    import("/src/desktop/widgets/app/fixture/fixture-plugin.ts"),
    import("/src/desktop/widgets/app/model/sandbox-methods.ts"),
  ])
  let bridge
  const reading = []
  let apiAccepted = false
  let canceled = false
  let errorKind = null
  const api = createMcpAppsApi(
    {
      request() {
        throw new Error("fixture does not use RPC")
      },
    },
    fetchMcpResource(input.url, (url, init) => {
      const promise = fetch(url, init)
      if (input.control === "remove-during-fetch") setTimeout(() => bridge.remove(), 0)
      return promise
    }),
    () => crypto.randomUUID(),
    (ms, elapsed) => {
      const handle = setTimeout(elapsed, ms)
      return () => clearTimeout(handle)
    },
  )
  const digest = crypto.subtle.digest
  if (input.control === "remove-during-digest")
    crypto.subtle.digest = function (...args) {
      bridge.remove()
      return Reflect.apply(digest, this, args)
    }
  try {
    const call = fixtureCall("synthetic-resource-session")
    const bridgeOptions = {
      place: "inline",
      server: "synthetic",
      call,
      context: {
        theme: "light",
        locale: "en-US",
        size: { width: 400, height: 300 },
        safeArea: { top: 0, right: 0, bottom: 0, left: 0 },
      },
      ports: {
        newId: () => crypto.randomUUID(),
        sandbox: { url: `${location.origin}/proxy.html`, origin: location.origin },
        hostInfo: { name: "Synthetic", version: "test" },
        calls: { read: () => ({ kind: "missing" }), subscribe: () => () => {} },
        timers: {
          after: (ms, elapsed) => {
            const handle = setTimeout(elapsed, ms)
            return () => clearTimeout(handle)
          },
        },
        page: () => ({ styles: {}, timeZone: "UTC", platform: "desktop" }),
        server: {
          readResource(_address, uri, signal) {
            signal.addEventListener(
              "abort",
              () => {
                canceled = true
              },
              { once: true },
            )
            const token = reading.length === 0 ? input.ticket : "V".repeat(43)
            const result = api.fetchResource(token, input.expected, { signal }).then(
              (bytes) => {
                apiAccepted = true
                return {
                  kind: "ok",
                  result: {
                    contents: [
                      {
                        uri,
                        mimeType: "text/html;profile=mcp-app",
                        text: new TextDecoder().decode(bytes),
                        _meta: { ui: {} },
                      },
                    ],
                  },
                }
              },
              (error) => {
                errorKind = ["integrity", "aborted", "unreachable", "timeout"].includes(
                  error.code,
                )
                  ? error.code
                  : "other"
                return { kind: "failed" }
              },
            )
            reading.push(result)
            return result
          },
          release: async () => {},
          callTool: async () => ({ kind: "failed" }),
          callWithin: 1000,
        },
      },
      post() {},
      host: { open() {}, close() {} },
      onView() {},
    }
    bridge = createAppBridge(bridgeOptions)
    const second = input.control === "two-mounts" ? createAppBridge(bridgeOptions) : null
    await Promise.all(reading)
    // Let the original bridge's read reaction run, then use its typed events.
    await Promise.resolve()
    for (const mounted of [bridge, second].filter(Boolean))
      if (mounted.view().lifecycle.kind === "proxy") {
        mounted.receive({
          kind: "notification",
          notification: { method: sandboxMethods.proxyReady },
        })
        mounted.receive({
          kind: "request",
          id: 1,
          request: {
            method: "ui/initialize",
            initialize: { protocolVersion: "2026-01-26", appName: "Synthetic" },
          },
        })
        mounted.receive({
          kind: "notification",
          notification: { method: "ui/notifications/initialized" },
        })
      }
    if (["remove-after-live", "two-mounts"].includes(input.control)) bridge.remove()
    if (["unrelated-request", "duplicate-request"].includes(input.control)) {
      await fetch(input.url, {
        headers: {
          "x-nessa-resource-ticket":
            input.control === "duplicate-request" ? input.ticket : "U".repeat(43),
        },
      }).then((response) => response.arrayBuffer())
    }
    const outcome = {
      apiAccepted,
      canceled,
      state: bridge.view().lifecycle.kind,
      errorKind,
      secondState: second?.view().lifecycle.kind ?? null,
    }
    document.body.textContent = `Synthetic resource control: ${input.control}; ${outcome.state}`
    return outcome
  } finally {
    crypto.subtle.digest = digest
  }
}

async function resourceControl(browser, origin, control) {
  const body = Buffer.from(
    `<html><body>${"synthetic ".repeat(control === "large-complete" ? 300000 : 64)}</body></html>`,
  )
  const ticket = "T".repeat(43)
  const served =
    control === "corrupt"
      ? Buffer.from(body.map((byte, index) => (index === 15 ? byte ^ 1 : byte)))
      : control === "truncated"
        ? body.subarray(0, body.length - 1)
        : body
  const server = createServer((request, response) => {
    response.setHeader("Access-Control-Allow-Origin", origin)
    response.setHeader("Access-Control-Allow-Headers", "x-nessa-resource-ticket")
    if (request.method === "OPTIONS") {
      response.writeHead(204)
      response.end()
      return
    }
    response.writeHead(200, {
      "Content-Type": "text/html",
      "Content-Length": served.length,
    })
    if (control === "remove-during-fetch") {
      response.write(served.subarray(0, 1))
      const timer = setTimeout(() => response.end(served.subarray(1)), 500)
      response.on("close", () => clearTimeout(timer))
    } else response.end(served)
  })
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve))
  const context = await browser.newContext()
  let observed
  return withCleanup(
    async () => {
      const page = await context.newPage()
      observed = await installResourceObservation(page)
      const requests = []
      const failureKinds = []
      page.on("requestfailed", (request) => {
        if (failureKinds.length < 16) failureKinds.push(request.failure()?.errorText)
      })
      page.on("request", (request) => {
        if (resourceEndpoint(request)) requests.push(request)
      })
      const fixture = `${origin}/verification/desktop/fixtures/resource-read/index.html`
      await page.goto(fixture)
      const outcome = await page.evaluate(readFixture, {
        control,
        ticket,
        expected: {
          size: body.length,
          sha256: createHash("sha256").update(body).digest("hex"),
        },
        url: `http://127.0.0.1:${server.address().port}/mcp-resources`,
      })
      await settleSizeReports([enqueueSizeReport(observed.finalize(), () => {})])
      observed.finish()
      assert.equal(observed.collector.summary().cutoff, true)
      const positive = ["complete", "large-complete", "unrelated-request"].includes(
        control,
      )
      assert.equal(
        observed.collector.readState(requests[0]),
        positive ? "verified" : "invalid",
        JSON.stringify({ outcome, evidence: observed.collector.summary(), failureKinds }),
      )
      if (control === "two-mounts") {
        assert.equal(
          observed.collector.readState(requests[1]),
          "verified",
          "canceling the first real mount must preserve the second",
        )
        assert.equal(outcome.secondState, "live")
        assert.equal(observed.collector.summary().verified, 1)
      }
      const classification = { errors: [], harmless: [] }
      // Deliberately simulated ERR_ABORTED on the exact actual Request. This is
      // classifier coverage, never evidence of a naturally occurring engine abort.
      const failed = requests[0].failure
      requests[0].failure = () => ({ errorText: "net::ERR_ABORTED" })
      try {
        classifyResourceFailure(requests[0], observed.collector, classification)
      } finally {
        requests[0].failure = failed
      }
      assert.equal(classification.harmless.length, positive ? 1 : 0)
      if (control === "unrelated-request") {
        assert.equal(observed.collector.readState(requests[1]), "unobserved")
        const unrelated = { errors: [], harmless: [] }
        requests[1].failure = () => ({ errorText: "net::ERR_ABORTED" })
        classifyResourceFailure(requests[1], observed.collector, unrelated)
        assert.equal(unrelated.errors.length, 1)
      }
      if (["corrupt", "truncated"].includes(control))
        assert.equal(outcome.apiAccepted, false)
      if (["remove-during-digest", "remove-after-live"].includes(control))
        assert.equal(
          outcome.apiAccepted,
          true,
          "late mount cancellation must revoke proof even when the original client accepts its bytes",
        )
      if (control.startsWith("remove-")) assert.equal(outcome.canceled, true)
      return {
        outcome,
        evidence: observed.collector.summary(),
        simulatedAbort: true,
        naturalAbortClaimed: false,
      }
    },
    async () => {
      observed?.finish()
      observed?.collector.dispose()
      await context.close()
      server.closeAllConnections()
      await new Promise((resolve) => server.close(resolve))
    },
  )
}

await main(
  {
    name: "resource-read-regression",
    summary: "bounded resource proof and failure-only animation capture",
    defaults: { engine: "chromium,webkit" },
    options: { evidence: { type: "string" } },
  },
  async ({ options, rep, target }) => {
    const origin = new URL(target.url).origin
    for (const engine of options.engines) {
      const browser = await launch(engine, options)
      await withCleanup(
        async () => {
          for (const control of controls) {
            try {
              rep.add({
                name: control,
                engine,
                seen: await resourceControl(browser, origin, control),
                failures: [],
              })
            } catch (error) {
              rep.add({ name: control, engine, failures: [error.message] })
            }
          }
          const context = await browser.newContext()
          await withCleanup(
            async () => {
              const page = await context.newPage()
              const tracker = watchOpenRequests(page)
              await page.goto(`${origin}/desktop.html`)
              await page.waitForSelector(css.workspace)
              await page.evaluate(() =>
                document.body.animate([{ opacity: 1 }, { opacity: 0.99 }], {
                  duration: 60000,
                }),
              )
              const startedAt = Date.now()
              let original
              try {
                await settled(page)
              } catch (error) {
                original = error
              }
              assert.ok(
                original,
                "the unchanged 3000ms finite-animation predicate must fail",
              )
              const root = options.evidence ?? options.shots
              if (root) await mkdir(root, { recursive: true })
              const directory = await captureOpenFailure(
                page,
                tracker,
                { ...options, evidence: root ? join(root, engine) : undefined },
                {
                  lastCompleted: "windowOpened-control",
                  wait: "settled",
                  startedAt,
                  failedAt: Date.now(),
                },
              )
              assert.ok(directory, "failure capture must exist before close")
              tracker.dispose()
              rep.add({
                name: "finite-animation-failure",
                engine,
                seen: {
                  originalFailure: original.name,
                  elapsedMs: Date.now() - startedAt,
                  directory,
                  timeoutChanged: false,
                },
                failures: [],
              })
            },
            () => context.close(),
          )
        },
        () => browser.close(),
      )
    }
  },
  (options) => startDevServer(options),
)
