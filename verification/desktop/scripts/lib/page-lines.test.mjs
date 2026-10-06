import assert from "node:assert/strict"
import { readdirSync, readFileSync, statSync } from "node:fs"
import { dirname, join } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"

import { report } from "./cli.mjs"
import {
  applyLines,
  attachLines,
  bindReporter,
  reportDetached,
  skipLineDrain,
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
    noteHeldHarmless: () => lines.noteHeldHarmless(),
    close: async () => {
      closed += 1
    },
    closed: () => closed,
  }
  return opened
}

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
  assert.equal(skipped.results.every((result) => result.failures.length === 0), true)
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
