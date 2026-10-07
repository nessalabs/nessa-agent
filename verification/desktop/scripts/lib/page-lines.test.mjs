import assert from "node:assert/strict"
import { readdirSync, readFileSync, statSync } from "node:fs"
import { dirname, join } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"

import { report } from "./cli.mjs"
import { noteLiveMountResourceAbort } from "./browser.mjs"
import {
  applyLines,
  attachLines,
  bindReporter,
  reportDetached,
  watchLines,
} from "./page-lines.mjs"

const here = dirname(fileURLToPath(import.meta.url))

function page() {
  let closed = 0
  const lines = watchLines()
  const opened = {
    errors: lines.errors,
    harmless: lines.harmless,
    noteHarmless: (pattern) => lines.noteHarmless(pattern),
    reclassifyHeld: (keep, asHarmless) => lines.reclassifyHeld(keep, asHarmless),
    close: async () => {
      closed += 1
    },
    closed: () => closed,
  }
  return opened
}

test("#647: live mount proof classifies a late abort before close, keeping rejected neighbors", async () => {
  const rep = report("page-lines-live-mount", {})
  bindReporter(rep)
  const lines = watchLines()
  let closed = 0
  const opened = {
    ...lines,
    page: { url: () => "http://127.0.0.1:1438/desktop.html" },
    close: async () => {
      closed += 1
    },
  }
  attachLines(opened, { engine: "chromium" })
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
      ...lines,
      page: { url: () => "http://127.0.0.1:1438/desktop.html" },
      close: async () => {},
    }
    attachLines(opened, {})
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
  attachLines(opened, { engine: "chromium", layout: "columns" })
  opened.errors.push("console.error: during the step")
  rep.add({ name: "step", failures: ["an assertion"] })
  assert.deepEqual(rep.results[0].failures, [
    "an assertion",
    "console.error: during the step",
  ])
  assert.equal(opened.errors.length, 0)

  opened.errors.push("console.error: between steps")
  opened.harmless.push("requestfailed: kept")
  rep.add({ name: "next" })
  assert.deepEqual(rep.results[1].failures, ["console.error: between steps"])
  assert.deepEqual(rep.results[1].harmless, ["requestfailed: kept"])

  opened.errors.push("console.error: after the last step")
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
  attachLines(held, { engine: "webkit" })
  held.errors.push("console.error: while opening")
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
  assert.deepEqual(held.errors, ["console.error: while opening"])
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
    errors: watched.errors,
    harmless: watched.harmless,
    close: async () => {},
  }
  attachLines(noisy, {})
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
  attachLines(mount, {})
  mount.errors.push("requestfailed: https://page/mcp-resources net::ERR_ABORTED")
  mount.errors.push("console.error: real")
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
    attachLines(opened, { engine: "chromium" })
    opened.errors.push("console.error: chromium")
    chromiumAttached()
    await webkitReady
    concurrent.add({ name: "chromium" })
    chromiumAdded()
    return opened
  })()
  const webkit = (async () => {
    await chromiumReady
    const opened = page()
    attachLines(opened, { engine: "webkit" })
    opened.errors.push("console.error: webkit")
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
  assert.equal(chromiumPage.errors.length, 0)
  assert.equal(webkitPage.errors.length, 0)
  await chromiumPage.close()
  await webkitPage.close()
})

test("scripts do not splice a page's line arrays", () => {
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
  const offenders = []
  for (const path of files) {
    if (path.endsWith(`${join("lib", "page-lines.mjs")}`)) continue
    if (path.endsWith(".test.mjs")) continue
    const source = readFileSync(path, "utf8")
    if (source.includes(".errors.splice") || source.includes(".harmless.splice"))
      offenders.push(path)
  }
  assert.deepEqual(offenders, [])
})
