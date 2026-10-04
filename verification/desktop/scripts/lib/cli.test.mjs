/**
 * The verification scripts' own contract, without a browser: how arguments
 * are read, which exit status a set of results earns, and how `run-all`
 * sums its checks. Run with `node --test verification/desktop/scripts/lib/`.
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import {
  attempt,
  CannotRun,
  chosen,
  overallStatus,
  parseOptions,
  resultOfThrown,
  statusOf,
  UsageError,
  verdictOf,
} from "./cli.mjs"

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
})
