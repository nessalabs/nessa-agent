import assert from "node:assert/strict"
import { readdirSync, statSync } from "node:fs"
import { dirname, join } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"
import ts from "typescript"
import { EventEmitter } from "node:events"
import { createHash } from "node:crypto"

import { report } from "./cli.mjs"
import { noteLiveMountResourceAbort, openPage } from "./browser.mjs"
import {
  applyLines,
  attachLines,
  bindReporter,
  reportDetached,
  watchLines,
} from "./page-lines.mjs"

const here = dirname(fileURLToPath(import.meta.url))
const collectors = new WeakMap()

test("close drains ordinary lines after its evidence barrier and reports only once", async () => {
  const rep = report("cutoff-lines", {})
  bindReporter(rep)
  const lines = watchLines()
  let release
  const pending = new Promise((resolve) => {
    release = resolve
  })
  let retired = 0
  const opened = {
    settleBeforeClose: () => pending,
    close: async () => {
      retired++
    },
  }
  attachLines(opened, { engine: "chromium" }, lines)
  const closing = opened.close()
  lines.keep("pageerror: while the acknowledged cutoff is pending", false)
  assert.equal(rep.results.length, 0)
  release()
  await closing
  await opened.close()
  assert.equal(retired, 1)
  assert.equal(rep.results.length, 1)
  assert.deepEqual(rep.results[0].failures, [
    "pageerror: while the acknowledged cutoff is pending",
  ])
})

function resourcePage({
  canceled = false,
  openingFailure = false,
  query = false,
  late = null,
  disposalAbort = false,
} = {}) {
  const page = new EventEmitter()
  const frame = {}
  const bytes = [1, 2, 3]
  const ticket = "F".repeat(43)
  let binding
  let closed = 0
  let finalized = 0
  const request = {
    method: () => "GET",
    frame: () => frame,
    url: () => `http://127.0.0.1:2345/mcp-resources${query ? "?unrelated=1" : ""}`,
    headers: () => ({ "x-nessa-resource-ticket": ticket }),
    failure: () => ({ errorText: "net::ERR_ABORTED" }),
    existingResponse: () => ({
      status: () => 200,
      headers: () => ({ "content-length": "3" }),
    }),
    sizes: () => {
      throw new Error("a retained candidate must never consult legacy sizes")
    },
  }
  const unrelated = {
    ...request,
    headers: () => ({ "x-nessa-resource-ticket": "U".repeat(43) }),
  }
  page.mainFrame = () => frame
  page.url = () => "http://127.0.0.1:2345/desktop.html"
  page.exposeBinding = async (_name, callback) => {
    binding = callback
  }
  page.route = async () => {}
  page.goto = async () => {
    page.emit("framenavigated", frame)
    page.emit("request", request)
    if (!late) page.emit("requestfailed", request)
    if (late === "unowned") page.emit("request", unrelated)
  }
  page.waitForSelector = async () => {
    if (openingFailure) throw new Error("synthetic opening fault")
  }
  page.waitForFunction = async () => {}
  page.evaluate = async (_fn, url) => {
    finalized++
    const generation = url.match(/resource_(.*)\.mjs$/)[1]
    const events = [
      {
        type: "begin",
        read: 1,
        owner: 1,
        ticket,
        size: bytes.length,
        digest: createHash("sha256").update(Buffer.from(bytes)).digest("hex"),
      },
      { type: "chunk", read: 1, bytes },
      { type: "eof", read: 1 },
      { type: "transport", read: 1 },
      { type: "accepted", read: 1 },
      { type: "live", read: 1 },
      ...(canceled ? [{ type: "failed", read: 1 }] : []),
      { type: "cutoff", failed: false, activation: { bridge: 1, api: 1, transport: 1 } },
    ]
    for (const [index, event] of events.entries())
      binding({ page, frame }, { generation, sequence: index + 1, ...event })
    if (!late) page.emit("pageerror", new Error("late ordinary fault"))
  }
  const context = {
    addInitScript: async () => {},
    newPage: async () => page,
    close: async () => {
      closed++
      if (disposalAbort) page.emit("requestfailed", unrelated)
    },
  }
  const browser = {
    browserType: () => ({ name: () => "chromium" }),
    newContext: async () => context,
  }
  return {
    browser,
    closed: () => closed,
    finalized: () => finalized,
    deliverLateFailure: () =>
      page.emit("requestfailed", late === "unowned" ? unrelated : request),
  }
}

for (const late of ["owned", "unowned"])
  test(`R1: independently delivered ${late} failure after cutoff ACK reaches the final synchronous drain`, async () => {
    const fixture = resourcePage({ late, disposalAbort: true })
    const rep = report("late-network-delivery", {})
    bindReporter(rep)
    const opened = await openPage(fixture.browser, {
      url: "http://127.0.0.1:2345/desktop.html",
      resourceReads: true,
    })
    const acknowledged = opened.settleBeforeClose()
    acknowledged.then(() => fixture.deliverLateFailure())
    await opened.close()
    await opened.close()
    const result = rep.results[0]
    assert.equal(
      result.ok,
      late === "owned",
      "a lost unrelated failure must not leave a success-only report",
    )
    assert.equal(
      result.failures.filter((line) => line.startsWith("requestfailed:")).length,
      late === "unowned" ? 1 : 0,
    )
    assert.equal(
      (result.harmless ?? []).some((line) => line.includes("#996")),
      late === "owned",
    )
    assert.equal(
      opened.heldErrorCount,
      0,
      "late notifications and disposal-induced aborts cannot enter an abandoned deferred set",
    )
    assert.equal(fixture.closed(), 1)
    assert.equal(fixture.finalized(), 1)
  })

test("observed endpoint Requests bypass step drains, patterns and size rules until final cutoff", async () => {
  for (const canceled of [false, true]) {
    const fixture = resourcePage({ canceled })
    const rep = report("retained-resource", {})
    bindReporter(rep)
    const opened = await openPage(fixture.browser, {
      url: "http://127.0.0.1:2345/desktop.html",
      resourceReads: true,
    })
    opened.noteHarmless(/mcp-resources/)
    rep.add({ name: "apps" })
    assert.deepEqual(rep.results[0].failures, [])
    assert.equal(rep.results[0].harmless, undefined)
    assert.equal(opened.heldErrorCount, 1)
    await opened.close()
    await opened.close()
    assert.equal(fixture.closed(), 1)
    assert.equal(fixture.finalized(), 1)
    const result = rep.results[1]
    assert.ok(result.failures.includes("pageerror: late ordinary fault"))
    assert.equal(
      result.failures.some((line) => line.startsWith("requestfailed:")),
      canceled,
    )
    assert.equal(
      (result.harmless ?? []).some((line) => line.includes("#996")),
      !canceled,
    )
  }
})

test("opening abandonment finalizes evidence before detached reporting and retirement", async () => {
  const fixture = resourcePage({ openingFailure: true, canceled: true })
  const rep = report("retained-resource-abandon", {})
  bindReporter(rep)
  await assert.rejects(
    openPage(fixture.browser, {
      url: "http://127.0.0.1:2345/desktop.html",
      resourceReads: true,
    }),
    /synthetic opening fault/,
  )
  assert.equal(fixture.closed(), 1)
  assert.equal(fixture.finalized(), 1)
  assert.ok(rep.results[0].failures.some((line) => line.startsWith("requestfailed:")))
})

test("reporter-absent retirement still finalizes exactly once", async () => {
  bindReporter(null)
  const fixture = resourcePage()
  const opened = await openPage(fixture.browser, {
    url: "http://127.0.0.1:2345/desktop.html",
    resourceReads: true,
  })
  await opened.close()
  await opened.close()
  assert.equal(fixture.closed(), 1)
  assert.equal(fixture.finalized(), 1)
})

test("an unrelated query endpoint cannot borrow the old live pattern", async () => {
  const fixture = resourcePage({ query: true })
  const rep = report("retained-resource-query", {})
  bindReporter(rep)
  const opened = await openPage(fixture.browser, {
    url: "http://127.0.0.1:2345/desktop.html",
    resourceReads: true,
  })
  opened.noteHarmless(/mcp-resources/)
  await opened.close()
  assert.ok(rep.results[0].failures.some((line) => line.includes("?unrelated=1")))
})

function page() {
  let closed = 0
  const lines = watchLines()
  const opened = {
    get heldErrorCount() {
      return lines.errors.length
    },
    noteHarmless: (pattern) => lines.noteHarmless(pattern),
    reclassifyHeld: (keep, asHarmless) => lines.reclassifyHeld(keep, asHarmless),
    close: async () => {
      closed += 1
    },
    closed: () => closed,
  }
  collectors.set(opened, lines)
  return opened
}

test("#647: live mount proof classifies a late abort before close, keeping rejected neighbors", async () => {
  const rep = report("page-lines-live-mount", {})
  bindReporter(rep)
  const lines = watchLines()
  let closed = 0
  const opened = {
    noteHarmless: (pattern) => lines.noteHarmless(pattern),
    page: { url: () => "http://127.0.0.1:1438/desktop.html" },
    close: async () => {
      closed += 1
    },
  }
  attachLines(opened, { engine: "chromium" }, lines)
  const abort = "requestfailed: http://127.0.0.1:1438/mcp-resources net::ERR_ABORTED"
  lines.keep(abort, false)
  noteLiveMountResourceAbort(opened, "live")
  rep.add({ name: "apps", seen: { state: "live" } })
  assert.deepEqual(rep.results[0].harmless, [abort])
  assert.deepEqual(rep.results[0].failures, [])
  // Delivered after apps consumed its held lines, before close drains them.
  lines.keep(abort, false)
  lines.keep(abort.replace("mcp-resources", "mcp-resources?x=1#mount"), false)
  const rejected = [
    abort.replace(":1438", ":1439"),
    abort.replace("/mcp-resources", "/other/mcp-resources"),
    abort.replace("ERR_ABORTED", "ERR_FAILED"),
    "console.error: real",
  ]
  for (const line of rejected) lines.keep(line, false)
  await opened.close()
  assert.deepEqual(rep.results[1].failures, rejected)
  assert.deepEqual(rep.results[1].harmless, [
    abort,
    abort.replace("mcp-resources", "mcp-resources?x=1#mount"),
  ])
  assert.equal(rep.results[1].name, "console")
  await opened.close()
  assert.equal(closed, 1)
  assert.equal(rep.results.length, 2)
})

test("#647: an absent live mount proof leaves held and late aborts as failures", async () => {
  for (const state of [undefined, "loading", "failed"]) {
    const rep = report("page-lines-no-live-mount", {})
    bindReporter(rep)
    const lines = watchLines()
    const opened = {
      noteHarmless: (pattern) => lines.noteHarmless(pattern),
      page: { url: () => "http://127.0.0.1:1438/desktop.html" },
      close: async () => {},
    }
    attachLines(opened, {}, lines)
    const abort = "requestfailed: http://127.0.0.1:1438/mcp-resources net::ERR_ABORTED"
    lines.keep(abort, false)
    noteLiveMountResourceAbort(opened, state)
    rep.add({ name: "apps" })
    assert.deepEqual(rep.results[0].failures, [abort], state)
    lines.keep(abort, false)
    await opened.close()
    assert.deepEqual(rep.results[1].failures, [abort], state)
    assert.equal(
      rep.results.every((entry) => entry.harmless === undefined),
      true,
    )
  }
})

test("a page's lines are reported once, on the result that follows them", async () => {
  const rep = report("page-lines", {})
  bindReporter(rep)
  const opened = page()
  attachLines(opened, { engine: "chromium", layout: "columns" }, collectors.get(opened))
  collectors.get(opened).keep("console.error: during the step", false)
  rep.add({ name: "step", failures: ["an assertion"] })
  assert.deepEqual(rep.results[0].failures, [
    "an assertion",
    "console.error: during the step",
  ])
  assert.equal(opened.heldErrorCount, 0)

  collectors.get(opened).keep("console.error: between steps", false)
  collectors.get(opened).keep("requestfailed: kept", true)
  rep.add({ name: "next" })
  assert.deepEqual(rep.results[1].failures, ["console.error: between steps"])
  assert.deepEqual(rep.results[1].harmless, ["requestfailed: kept"])

  collectors.get(opened).keep("console.error: after the last step", false)
  await opened.close()
  const late = rep.results.at(-1)
  assert.equal(late.name, "console")
  assert.equal(late.engine, "chromium")
  assert.deepEqual(late.failures, ["console.error: after the last step"])
  assert.equal(opened.closed(), 1)
  await opened.close()
  assert.equal(opened.closed(), 1)
  assert.equal(rep.results.filter((result) => result.name === "console").length, 1)

  const skipped = report("page-lines-skip", {})
  bindReporter(skipped)
  const held = page()
  attachLines(held, { engine: "webkit" }, collectors.get(held))
  collectors.get(held).keep("console.error: while opening", false)
  skipped.add({ name: "open", cannotRun: true, error: "could not run" })
  skipped.add({
    name: "later",
    cannotRun: true,
    error: "not run: the page did not open",
  })
  assert.equal(
    skipped.results.every((result) => result.failures.length === 0),
    true,
  )
  assert.deepEqual(collectors.get(held).errors, ["console.error: while opening"])
  await held.close()
  assert.deepEqual(skipped.results.at(-1).failures, ["console.error: while opening"])

  const detached = report("page-lines-open", {})
  bindReporter(detached)
  const errors = ["pageerror: boom"]
  const harmless = ["console.error: favicon"]
  reportDetached({ engine: "chromium" }, errors, harmless)
  assert.deepEqual(errors, [])
  assert.equal(detached.results[0].name, "console")
  assert.deepEqual(detached.results[0].failures, ["pageerror: boom"])
  assert.deepEqual(detached.results[0].harmless, ["console.error: favicon"])
  const again = applyLines({ name: "open", cannotRun: true })
  assert.equal(again.failures, undefined)

  const moved = report("page-lines-harmless", {})
  bindReporter(moved)
  const watched = watchLines()
  const noisy = {
    close: async () => {},
  }
  attachLines(noisy, {}, watched)
  watched.keep("requestfailed: https://page/browser/check net::ERR_ABORTED", false)
  watched.keep("console.error: real", false)
  watched.noteHarmless(/\/browser\/check net::ERR_ABORTED/)
  watched.keep("requestfailed: https://page/browser/check net::ERR_ABORTED", false)
  moved.add({ name: "step" })
  assert.deepEqual(moved.results[0].failures, ["console.error: real"])
  assert.deepEqual(moved.results[0].harmless, [
    "requestfailed: https://page/browser/check net::ERR_ABORTED",
    "requestfailed: https://page/browser/check net::ERR_ABORTED",
  ])

  const live = report("page-lines-reclassify", {})
  bindReporter(live)
  const mount = page()
  attachLines(mount, {}, collectors.get(mount))
  collectors
    .get(mount)
    .keep("requestfailed: https://page/mcp-resources net::ERR_ABORTED", false)
  collectors.get(mount).keep("console.error: real", false)
  mount.reclassifyHeld(
    (line) => line.includes("/mcp-resources"),
    (line) => `${line} (mount went live, #473)`,
  )
  live.add({ name: "release" })
  assert.deepEqual(live.results[0].failures, ["console.error: real"])
  assert.deepEqual(live.results[0].harmless, [
    "requestfailed: https://page/mcp-resources net::ERR_ABORTED (mount went live, #473)",
  ])
  await mount.close()

  // Two pages stay open together. The older page's result is added while the
  // newer page is the one opened last, and each result keeps its own line.
  const concurrent = report("page-lines-concurrent", {})
  bindReporter(concurrent)
  let chromiumAttached
  const chromiumReady = new Promise((resolve) => {
    chromiumAttached = resolve
  })
  let webkitAttached
  const webkitReady = new Promise((resolve) => {
    webkitAttached = resolve
  })
  let chromiumAdded
  const chromiumDone = new Promise((resolve) => {
    chromiumAdded = resolve
  })
  const chromium = (async () => {
    const opened = page()
    attachLines(opened, { engine: "chromium" }, collectors.get(opened))
    collectors.get(opened).keep("console.error: chromium", false)
    chromiumAttached()
    await webkitReady
    concurrent.add({ name: "chromium" })
    chromiumAdded()
    return opened
  })()
  const webkit = (async () => {
    await chromiumReady
    const opened = page()
    attachLines(opened, { engine: "webkit" }, collectors.get(opened))
    collectors.get(opened).keep("console.error: webkit", false)
    webkitAttached()
    await chromiumDone
    concurrent.add({ name: "webkit" })
    return opened
  })()
  const [chromiumPage, webkitPage] = await Promise.all([chromium, webkit])
  const byName = Object.fromEntries(
    concurrent.results.map((result) => [result.name, result]),
  )
  assert.deepEqual(byName.chromium.failures, ["console.error: chromium"])
  assert.deepEqual(byName.webkit.failures, ["console.error: webkit"])
  assert.equal(chromiumPage.heldErrorCount, 0)
  assert.equal(webkitPage.heldErrorCount, 0)
  await chromiumPage.close()
  await webkitPage.close()
})

test("openPage publishes counts and classification actions without diagnostic queues", async () => {
  const rep = report("page-lines-public-api", {})
  bindReporter(rep)
  const handlers = new Map()
  let closed = 0
  const context = {
    addInitScript: async () => {},
    newPage: async () => ({
      on: (name, receive) => handlers.set(name, receive),
      goto: async () => {},
      waitForSelector: async () => {},
      waitForFunction: async () => {},
    }),
    close: async () => {
      closed += 1
    },
  }
  const opened = await openPage(
    { browserType: () => ({ name: () => "chromium" }), newContext: async () => context },
    { url: "http://page/", width: 600, lines: { width: 600 } },
  )
  assert.equal(Object.hasOwn(opened, "errors"), false)
  assert.equal(Object.hasOwn(opened, "harmless"), false)
  assert.equal(opened.heldErrorCount, 0)
  const error = { type: () => "error", text: () => "same fault", location: () => ({}) }
  handlers.get("console")(error)
  handlers.get("console")(error)
  assert.equal(opened.heldErrorCount, 2)
  await opened.settleRequests()
  await opened.close()
  await opened.close()
  assert.equal(opened.heldErrorCount, 0)
  assert.equal(closed, 1)
  assert.equal(rep.results.length, 1)
  assert.deepEqual(rep.results[0].failures, [
    "console.error: same fault",
    "console.error: same fault",
  ])
  assert.equal(rep.results[0].engine, "chromium")
  assert.equal(rep.results[0].width, 600)
})

test("close reports held diagnostics even when context retirement fails", async () => {
  const rep = report("page-lines-close-failure", {})
  bindReporter(rep)
  const lines = watchLines()
  const failure = new Error("context close failed")
  let closed = 0
  const opened = {
    close: async () => {
      closed += 1
      throw failure
    },
  }
  attachLines(opened, { engine: "webkit", width: 360 }, lines)
  lines.keep("pageerror: held before close", false)
  await assert.rejects(opened.close(), (error) => error === failure)
  await opened.close()
  assert.equal(closed, 1)
  assert.equal(rep.results.length, 1)
  assert.deepEqual(rep.results[0].failures, ["pageerror: held before close"])
  assert.equal(rep.results[0].engine, "webkit")
  assert.equal(rep.results[0].width, 360)
})

test("page consumers and wrappers do not access private queues through any handle name", () => {
  const root = join(here, "..")
  const files = []
  const walk = (directory) => {
    for (const name of readdirSync(directory)) {
      const path = join(directory, name)
      if (statSync(path).isDirectory()) walk(path)
      else if (name.endsWith(".mjs")) files.push(path)
    }
  }
  walk(root)
  const program = ts.createProgram(files, {
    allowJs: true,
    checkJs: true,
    noEmit: true,
    module: ts.ModuleKind.NodeNext,
    moduleResolution: ts.ModuleResolutionKind.NodeNext,
    target: ts.ScriptTarget.ESNext,
  })
  const checker = program.getTypeChecker()
  const browser = program.getSourceFile(join(here, "browser.mjs"))
  const declaration = browser.statements.find(
    (node) => ts.isFunctionDeclaration(node) && node.name?.text === "openPage",
  )
  const pageType = checker.getAwaitedType(
    checker.getReturnTypeOfSignature(checker.getSignatureFromDeclaration(declaration)),
  )
  const offenders = []
  for (const path of files) {
    if (path.endsWith(".test.mjs")) continue
    const source = program.getSourceFile(path)
    const visit = (node) => {
      const binding = ts.isBindingElement(node) && ts.isObjectBindingPattern(node.parent)
      const key = binding ? (node.propertyName ?? node.name) : null
      const property =
        key && (ts.isIdentifier(key) || ts.isStringLiteral(key))
          ? key.text
          : ts.isPropertyAccessExpression(node)
            ? node.name.text
            : ts.isElementAccessExpression(node) &&
                ts.isStringLiteral(node.argumentExpression)
              ? node.argumentExpression.text
              : null
      if (property === "errors" || property === "harmless") {
        const receiver = checker.getNonNullableType(
          checker.getTypeAtLocation(binding ? node.parent : node.expression),
        )
        if (
          !(receiver.flags & (ts.TypeFlags.Any | ts.TypeFlags.Unknown)) &&
          checker.isTypeAssignableTo(receiver, pageType)
        ) {
          const { line } = source.getLineAndCharacterOfPosition(node.getStart())
          offenders.push(`${path}:${line + 1}: ${node.getText(source)}`)
        }
      }
      ts.forEachChild(node, visit)
    }
    visit(source)
  }
  assert.deepEqual(offenders, [])
})
