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
  UnsafeToContinueError,
  UsageError,
  verdictOf,
} from "./cli.mjs"
import { argsForCheck, ownsPreview } from "./run-all-args.mjs"

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

  it("stops the check when recovery leaves its browser page unsafe to reuse", async () => {
    const rep = collect()
    const fatal = new UnsafeToContinueError("page retirement failed")
    await assert.rejects(
      attempt(rep, { name: "step" }, async () => {
        throw fatal
      }),
      (error) => error === fatal,
    )
    assert.deepEqual(rep.results, [])
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

  it("includes the original operation error in fatal recovery diagnostics", async () => {
    const operationFailure = new Error("opening key acknowledgement failed")
    const fatal = new UnsafeToContinueError("page retirement failed", {
      cause: operationFailure,
    })
    let result
    const written = await stderrOf(() => (result = resultOfThrown({ name: "s" }, fatal)))
    assert.match(written, /UnsafeToContinueError: page retirement failed/)
    assert.match(written, /caused by: Error: opening key acknowledgement failed/)
    assert.equal(
      result.error,
      "page retirement failed (caused by: Error: opening key acknowledgement failed)",
    )
  })

  it("retains a non-Error operation cause in the fatal result", async () => {
    const fatal = new UnsafeToContinueError("page retirement failed", {
      cause: "opening key acknowledgement failed",
    })
    let result
    const written = await stderrOf(() => (result = resultOfThrown({ name: "s" }, fatal)))
    assert.match(written, /caused by: opening key acknowledgement failed/)
    assert.equal(
      result.error,
      "page retirement failed (caused by: opening key acknowledgement failed)",
    )
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
  it("routes production Settings to its fixture preview owner", () => {
    assert.equal(ownsPreview("settings-page", "prod"), true)
    assert.equal(ownsPreview("settings-page", "dev"), false)
    assert.equal(ownsPreview("smoke", "prod"), false)
    assert.deepEqual(
      argsForCheck(
        "settings-page",
        {},
        {
          out: "settings.json",
          page: undefined,
          sharesPage: false,
        },
      ),
      ["--out", "settings.json", "--mode", "prod"],
    )
  })
  it("keeps explicit Settings URLs externally owned and perf runs out of functional arguments", () => {
    const url = "http://127.0.0.1:9000/desktop.html"
    for (const check of ["settings-page", "committed-transcript"])
      assert.deepEqual(
        argsForCheck(
          check,
          { url, runs: "3" },
          { out: "check.json", page: undefined, sharesPage: false },
        ),
        ["--out", "check.json", "--url", url, "--mode", "prod"],
      )
    assert.deepEqual(
      argsForCheck(
        "perf-budget",
        { url, runs: "3" },
        { out: "perf.json", page: undefined, sharesPage: false },
      ),
      ["--out", "perf.json", "--url", url, "--runs", "3"],
    )
    assert.deepEqual(
      argsForCheck(
        "message-sync",
        { url, runs: "3" },
        { out: "sync.json", page: undefined, sharesPage: false },
      ),
      ["--out", "sync.json", "--mode", "prod"],
    )
  })

  it("preserves the shared functional page mode and existing preview owners", () => {
    for (const mode of ["dev", "prod"]) {
      assert.equal(ownsPreview("message-sync", mode), true)
      assert.equal(ownsPreview("perf-budget", mode), true)
      assert.equal(ownsPreview("committed-transcript", mode), mode === "prod")
      assert.equal(ownsPreview("smoke", mode), false)
      assert.deepEqual(
        argsForCheck(
          "smoke",
          { runs: "3" },
          {
            base: ["--engine", "webkit"],
            out: "smoke.json",
            page: { url: "http://127.0.0.1:9000/desktop.html", mode },
            sharesPage: true,
          },
        ),
        [
          "--engine",
          "webkit",
          "--out",
          "smoke.json",
          "--url",
          "http://127.0.0.1:9000/desktop.html",
          "--mode",
          mode,
        ],
      )
    }
  })

  it("recognizes the clock check by --only name and leaves it out of production previews", () => {
    const source = readFileSync(new URL("../run-all.mjs", import.meta.url), "utf8")
    const match = source.match(/const functional = \[([\s\S]*?)\n\]/)
    assert.ok(match, "run-all declares its functional check registry")
    const functional = Array.from(match[1].matchAll(/"([^"]+)"/g), ([, name]) => name)
    assert.deepEqual(
      chosen("clock-snapshots", functional, (value) => value.split(",").filter(Boolean)),
      ["clock-snapshots"],
    )

    const scheduled = checksUnder("prod", functional)
    assert.ok(scheduled.leftOut.includes("clock-snapshots"))
    assert.ok(!scheduled.run.includes("clock-snapshots"))
  })

  it("passes an explicit production URL to committed-transcript", () => {
    const url = "http://127.0.0.1:9000/desktop.html"
    assert.deepEqual(
      argsForCheck(
        "committed-transcript",
        { url, runs: undefined },
        { base: [], out: "check.json", page: undefined, sharesPage: false },
      ),
      ["--out", "check.json", "--url", url, "--mode", "prod"],
    )
    assert.deepEqual(
      argsForCheck(
        "committed-transcript",
        { url: undefined },
        { base: [], out: "check.json", page: undefined, sharesPage: false },
      ),
      ["--out", "check.json", "--mode", "prod"],
    )
  })

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
      "panel-list-follow",
      "mode-publication",
      "local-tab-lifetimes",
      "lease-details",
      "app-review",
      "gateway-states",
      "published-files",
      "transcript-notes",
      "transcript-unreadable",
      "user-attachments",
      "turn-outcomes",
      "clock-snapshots",
      "mcp-apps",
      "approval-deadline",
      "send-failure",
      "refused-messages",
    ])
    assert.deepEqual(checksUnder("prod", ["smoke", ...devServerOnlyChecks]), {
      run: ["smoke"],
      leftOut: [...devServerOnlyChecks],
    })
    assert.deepEqual(checksUnder("dev", ["smoke", "app-review"]).leftOut, [])
    assert.deepEqual(checksUnder("prod", ["committed-transcript"]), {
      run: ["committed-transcript"],
      leftOut: [],
    })
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

describe("optional scenario assertions", () => {
  for (const [name, supplied, ok, status] of [
    ["omitted", {}, true, 0],
    ["derived success", { ok: false, failures: undefined }, true, 0],
    ["undefined", { failures: undefined }, true, 0],
    ["null", { failures: null }, true, 0],
    ["error", { error: "temporarily_unavailable", failures: undefined }, false, 1],
    ["assertion", { ok: true, failures: ["contract broke"] }, false, 1],
    [
      "error and assertion",
      { error: "original fault", failures: ["contract broke"] },
      false,
      1,
    ],
    [
      "cannot run",
      { cannotRun: true, error: "browser absent", failures: undefined },
      false,
      2,
    ],
    ["cannot run without diagnostic", { cannotRun: true, failures: undefined }, false, 2],
    ["skipped", { skipped: DEV_SERVER_ONLY, failures: undefined }, false, 2],
  ])
    it(`preserves the ${name} verdict and original diagnostics`, async () => {
      const rep = report("scenario", {})
      let entry
      const lines = await stderrOf(() => {
        entry = rep.add({ name, ...supplied })
      })
      assert.equal(entry.ok, ok)
      assert.deepEqual(entry.failures, supplied.failures ?? [])
      assert.equal(entry.error, supplied.error)
      assert.equal(entry.skipped, supplied.skipped)
      assert.equal(statusOf(rep.results), status)
      if (supplied.cannotRun) assert.match(lines, /^ERROR/)
      if (supplied.error) assert.ok(lines.includes(supplied.error))
      for (const failure of supplied.failures ?? []) assert.ok(lines.includes(failure))
    })

  it("keeps the scenario error in JSON and stderr with a failing process status", () => {
    const result = spawnSync(
      process.execPath,
      [
        "--input-type=module",
        "--eval",
        `import { report } from ${JSON.stringify(new URL("./cli.mjs", import.meta.url).href)};
const rep = report("scenario", {});
rep.add({ name: "original", error: "temporarily_unavailable", failures: undefined });
process.exit(rep.finish());`,
      ],
      { encoding: "utf8" },
    )
    assert.equal(result.status, 1, result.stderr)
    const document = JSON.parse(result.stdout)
    assert.equal(document.ok, false)
    assert.equal(document.results[0].error, "temporarily_unavailable")
    assert.deepEqual(document.results[0].failures, [])
    assert.match(result.stderr, /temporarily_unavailable/)
    assert.doesNotMatch(result.stderr, /TypeError/)
  })
})
