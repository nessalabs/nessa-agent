/**
 * The verification scripts' own contract, without a browser: how arguments
 * are read, which exit status a set of results earns, and how `run-all`
 * sums its checks. Run with `node --test verification/desktop/scripts/lib/`.
 */
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { createRequire } from "node:module"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { describe, it } from "node:test"
import { fileURLToPath } from "node:url"

import {
  attempt,
  CannotRun,
  checksUnder,
  chosen,
  DEV_SERVER_ONLY,
  devServerOnlyChecks,
  devServerOnlySteps,
  overallStatus,
  parseOptions,
  recordIfLeftOut,
  report,
  resultOfThrown,
  statusOf,
  UsageError,
  verdictOf,
} from "./cli.mjs"

const require = createRequire(import.meta.url)

// What `log` writes to stderr while `body` runs; stderr is put back after.
const stderrOf = async (body) => {
  const written = []
  const write = process.stderr.write
  process.stderr.write = (chunk) => (written.push(String(chunk)), true)
  try {
    await body()
  } finally {
    process.stderr.write = write
  }
  return written.join("")
}

const meta = {
  name: "check",
  options: { only: { type: "string" }, sizes: { type: "string", default: "1x1,2x2" } },
  defaults: { engine: "chromium,webkit", layout: "columns,sidebar" },
}

describe("parseOptions", () => {
  it("takes the check's defaults, and lists split on commas", () => {
    const options = parseOptions(meta, [])
    assert.deepEqual(options.engines, ["chromium", "webkit"])
    assert.deepEqual(options.layouts, ["columns", "sidebar"])
    assert.deepEqual(options.choices("sizes"), ["1x1", "2x2"])
    assert.equal(options.mode, "dev")
  })

  it("narrows to the first of each list with --quick, unless one was asked for by name", () => {
    const quick = parseOptions(meta, ["--quick"])
    assert.deepEqual(quick.engines, ["chromium"])
    assert.deepEqual(quick.layouts, ["columns"])
    assert.deepEqual(quick.choices("sizes"), ["1x1"])
    const named = parseOptions(meta, ["--quick", "--engine", "webkit,chromium"])
    assert.deepEqual(named.engines, ["webkit", "chromium"])
    assert.equal(named.given("engine"), true)
    assert.equal(named.given("layout"), false)
  })

  it("drops the -- that pnpm forwards", () => {
    assert.equal(parseOptions(meta, ["--", "--headed"]).headed, true)
  })

  it("refuses what the check was not written for, as could-not-run", () => {
    assert.throws(() => parseOptions(meta, ["--nope"]), UsageError)
    assert.throws(() => parseOptions(meta, ["stray"]), UsageError)
    assert.ok(new UsageError("x") instanceof CannotRun)
  })

  it("says when help was asked for", () => {
    assert.equal(parseOptions(meta, ["-h"]).help, true)
  })
})

describe("chosen", () => {
  const list = (value) => value.split(",").filter(Boolean)
  it("is every check without --only, and the named ones with it, in order", () => {
    assert.deepEqual(chosen(undefined, ["a", "b"], list), ["a", "b"])
    assert.deepEqual(chosen("b,a", ["a", "b"], list), ["b", "a"])
  })

  it("refuses a name it does not have rather than skipping it", () => {
    assert.throws(() => chosen("a,zzz", ["a", "b"], list), /no check named zzz/)
    assert.throws(() => chosen("constructor", ["a"], list), UsageError)
  })
})

describe("statusOf", () => {
  const held = { name: "a", failures: [] }
  const failed = { name: "b", failures: ["broke"] }
  const errored = { name: "c", failures: [], error: "timed out waiting for the drop" }
  const couldNot = { name: "d", failures: [], cannotRun: true, error: "no server" }

  it("is 0 when everything held", () => {
    assert.equal(statusOf([held, held]), 0)
  })

  it("is 1 when any contract broke — a failure, or an error that is not could-not-run", () => {
    assert.equal(statusOf([held, failed]), 1)
    assert.equal(statusOf([held, errored]), 1)
  })

  it("stays 1 when something else could not run: a product failure is never exit 2", () => {
    assert.equal(statusOf([failed, couldNot]), 1)
    assert.equal(statusOf([couldNot, errored]), 1)
  })

  it("is 2 only when nothing broke and something could not run, or nothing ran", () => {
    assert.equal(statusOf([held, couldNot]), 2)
    assert.equal(statusOf([]), 2)
  })

  it("does not count a dev-server step a production run left out", () => {
    const leftOut = { name: "boundary-jitter", failures: [], skipped: DEV_SERVER_ONLY }
    assert.equal(statusOf([held, leftOut]), 0)
    assert.equal(statusOf([leftOut]), 2)
    assert.equal(statusOf([failed, leftOut]), 1)
  })
})

describe("attempt", () => {
  const collect = () => {
    const results = []
    return { results, add: (r) => (results.push(r), r) }
  }

  it("turns a timeout waiting on the product into a failure, not could-not-run", async () => {
    const rep = collect()
    // Its stack goes to stderr (`resultOfThrown`); kept out of the test's output.
    const written = await stderrOf(() =>
      attempt(rep, { name: "step" }, async () => {
        throw new Error("Timeout 3000ms exceeded.\nwaiting for locator('.x')")
      }),
    )
    // Once, by `resultOfThrown` alone.
    assert.equal(written.split("Error: Timeout 3000ms exceeded.").length - 1, 1)
    assert.equal(rep.results[0].cannotRun, false)
    assert.equal(rep.results[0].error, "Timeout 3000ms exceeded.")
    assert.equal(statusOf(rep.results), 1)
  })

  it("keeps could-not-run for what the step needs to begin", async () => {
    const rep = collect()
    await attempt(rep, { name: "step" }, async () => {
      throw new CannotRun("no pane header")
    })
    assert.equal(rep.results[0].cannotRun, true)
    assert.equal(statusOf(rep.results), 2)
  })

  it("adds what the step returned", async () => {
    const rep = collect()
    await attempt(rep, { name: "step", engine: "webkit" }, async () => ({
      failures: ["x"],
    }))
    assert.deepEqual(rep.results[0], { name: "step", engine: "webkit", failures: ["x"] })
  })

  it("reads an error that is not an Error", async () => {
    let result
    await stderrOf(() => (result = resultOfThrown({ name: "s" }, "plain")))
    assert.equal(result.error, "plain")
  })
})

describe("resultOfThrown", () => {
  it("writes a fault's stack to stderr, as the result keeps only its first line", async () => {
    const fault = new Error("broke\nsecond line")
    let result
    const written = await stderrOf(() => (result = resultOfThrown({ name: "s" }, fault)))
    assert.equal(written, `${fault.stack}\n`)
    assert.equal(result.error, "broke")
  })

  it("writes nothing for could-not-run, which is not a fault", async () => {
    const written = await stderrOf(() =>
      resultOfThrown({ name: "s" }, new CannotRun("no server")),
    )
    assert.equal(written, "")
  })

  it("writes the message of a thrown object that has no stack", async () => {
    const written = await stderrOf(() =>
      resultOfThrown({ name: "s" }, { message: "obj" }),
    )
    assert.equal(written, "obj\n")
  })
})

describe("run-all's sum", () => {
  it("reads each check's exit code", () => {
    assert.equal(verdictOf(0), "held")
    assert.equal(verdictOf(1), "FAILED")
    assert.equal(verdictOf(2), "COULD NOT RUN")
    // A crash or a signal did not say it could not run: it failed.
    assert.equal(verdictOf(null), "FAILED")
    assert.equal(verdictOf(134), "FAILED")
  })

  it("exits 1 when any check failed, whatever else could not run; 2 only when none failed", () => {
    assert.equal(overallStatus(["held", "held"]), 0)
    assert.equal(overallStatus(["held", "FAILED", "COULD NOT RUN"]), 1)
    assert.equal(overallStatus(["held", "COULD NOT RUN"]), 2)
    assert.equal(overallStatus([]), 2)
  })

  it("does not count a dev-server check a production run left out", () => {
    assert.equal(overallStatus(["held", DEV_SERVER_ONLY]), 0)
    assert.equal(overallStatus([DEV_SERVER_ONLY]), 2)
    assert.equal(overallStatus(["FAILED", DEV_SERVER_ONLY]), 1)
    assert.deepEqual(devServerOnlyChecks, [
      "committed-transcript",
      "app-review",
      "gateway-states",
      "mcp-apps",
    ])
    assert.deepEqual(checksUnder("prod", ["smoke", ...devServerOnlyChecks]), {
      run: ["smoke"],
      leftOut: [...devServerOnlyChecks],
    })
    assert.deepEqual(checksUnder("dev", ["smoke", "app-review"]).leftOut, [])
  })

  it("records a dev-server step as left out only under prod", () => {
    const rep = {
      results: [],
      add(result) {
        this.results.push(result)
        return result
      },
    }
    assert.equal(
      recordIfLeftOut(rep, "prod", "boundary-jitter", devServerOnlySteps.drag, {
        engine: "chromium",
      }),
      true,
    )
    assert.equal(rep.results[0].skipped, DEV_SERVER_ONLY)
    assert.equal(
      recordIfLeftOut(rep, "dev", "boundary-jitter", devServerOnlySteps.drag, {}),
      false,
    )
    assert.equal(recordIfLeftOut(rep, "prod", "card", devServerOnlySteps.drag, {}), false)
    assert.equal(rep.results.length, 1)
    const scripts = dirname(fileURLToPath(import.meta.url))
    for (const [file, needle] of [
      ["../drag.mjs", "devServerOnlySteps.drag"],
      ["../widgets.mjs", "devServerOnlySteps.widgets"],
      ["../mcp-apps.mjs", 'devServerOnlySteps["mcp-apps"]'],
    ])
      assert.match(
        readFileSync(join(scripts, file), "utf8"),
        new RegExp(needle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
      )
    // A production preview has no sandbox origin. The check says so before
    // it builds one, and before any fixture waits for a frame.
    assert.match(
      readFileSync(join(scripts, "../mcp-apps.mjs"), "utf8"),
      /absent from vite preview[\s\S]*return target\(options\)/,
    )
  })
})

describe("report", () => {
  it("writes --out JSON that prettier accepts", () => {
    const dir = mkdtempSync(join(tmpdir(), "nessa-json-"))
    const path = join(dir, "perf-budget.json")
    try {
      const rep = report("perf-budget", { out: path })
      rep.add({
        name: "drag-drop",
        failures: ["longest frame 66.69999999999982 ms > 50 ms (runs: 67)"],
      })
      assert.equal(rep.finish(), 1)
      const bin = require.resolve("prettier/bin/prettier.cjs")
      const result = spawnSync(process.execPath, [bin, "--check", path], {
        encoding: "utf8",
      })
      assert.equal(result.status, 0, result.stderr)
      const document = JSON.parse(readFileSync(path, "utf8"))
      assert.equal(
        document.results[0].failures[0],
        "longest frame 66.69999999999982 ms > 50 ms (runs: 67)",
      )
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
})

describe("report", () => {
  it("writes --out JSON that prettier accepts", () => {
    const dir = mkdtempSync(join(tmpdir(), "nessa-json-"))
    const path = join(dir, "perf-budget.json")
    try {
      const rep = report("perf-budget", { out: path })
      rep.add({
        name: "drag-drop",
        failures: ["longest frame 66.69999999999982 ms > 50 ms (runs: 67)"],
      })
      assert.equal(rep.finish(), 1)
      const bin = require.resolve("prettier/bin/prettier.cjs")
      const result = spawnSync(process.execPath, [bin, "--check", path], {
        encoding: "utf8",
      })
      assert.equal(result.status, 0, result.stderr)
      const document = JSON.parse(readFileSync(path, "utf8"))
      assert.equal(
        document.results[0].failures[0],
        "longest frame 66.69999999999982 ms > 50 ms (runs: 67)",
      )
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
})
