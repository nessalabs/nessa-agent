import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"
import { matchedSinglePass, repeatLoaded, runsPassed } from "./paired-reader-load.mjs"

test("a partial ledger and a nonzero exit cannot become a passing summary", () => {
  assert.equal(runsPassed([{ exit_code: 0, matched: true }], 10), false)
  assert.equal(runsPassed([{ exit_code: 1, matched: true }], 1), false)
  assert.equal(runsPassed([{ exit_code: 0, matched: true }], 1), true)
})

test("libtest must actually run one successful test, rather than match zero", () => {
  assert.equal(
    matchedSinglePass(
      "running 0 tests\ntest result: ok. 0 passed; 0 failed; 3185 filtered out",
    ),
    false,
  )
  assert.equal(
    matchedSinglePass("running 1 test\ntest result: FAILED. 0 passed; 1 failed;"),
    false,
  )
  assert.equal(
    matchedSinglePass(
      "running 1 test\ntest result: ok. 1 passed; 0 failed; 3184 filtered out",
    ),
    true,
  )
})

function fixture(results) {
  const state = { started: 0, stopped: 0, runs: [], ledgers: [] }
  return {
    state,
    dependencies: {
      repetitions: results.length,
      startLoad: async () => {
        state.started += 1
        return async () => {
          state.stopped += 1
        }
      },
      runTest: async (run) => {
        state.runs.push(run)
        if (results[run - 1] instanceof Error) throw results[run - 1]
        return results[run - 1]
      },
      save: (runs) => state.ledgers.push(structuredClone(runs)),
    },
  }
}

test("loaded repetitions retain early failures instead of accepting later passes", async () => {
  const { state, dependencies } = fixture([
    { exit_code: 1, matched: false },
    { exit_code: 0, matched: true },
    { exit_code: 0, matched: true },
  ])
  assert.equal(await repeatLoaded(dependencies), false)
  assert.deepEqual(state.runs, [1, 2, 3])
  assert.deepEqual(
    state.ledgers.map((runs) => runs.length),
    [1, 2, 3],
  )
  assert.equal(state.ledgers[2][0].exit_code, 1)
  assert.equal(state.started, 1)
  assert.equal(state.stopped, 1)
})

test("zero matched tests and failed process launches cannot pass", async () => {
  for (const result of [{ exit_code: 0, matched: false }, new Error("spawn failure")]) {
    const { state, dependencies } = fixture([result, { exit_code: 0, matched: true }])
    assert.equal(await repeatLoaded(dependencies), false)
    assert.equal(state.ledgers[1][0].matched, false)
    assert.equal(state.stopped, 1)
  }
})

test("successful runs pass and a failed ledger still releases the CPU workers", async () => {
  const success = fixture([{ exit_code: 0, matched: true }])
  assert.equal(await repeatLoaded(success.dependencies), true)
  assert.equal(success.state.stopped, 1)
  const failed = fixture([{ exit_code: 0, matched: true }])
  failed.dependencies.save = () => {
    throw new Error("synthetic ledger failure")
  }
  await assert.rejects(repeatLoaded(failed.dependencies), /synthetic ledger failure/)
  assert.equal(failed.state.stopped, 1)
})

test("repeat cost is bounded before starting load", async () => {
  for (const repetitions of [0, 11, 1.5]) {
    const { state, dependencies } = fixture([])
    await assert.rejects(
      repeatLoaded({ ...dependencies, repetitions }),
      /between 1 and 10/,
    )
    assert.equal(state.started, 0)
  }
})

test("existing macOS matrix owns the loaded check, same package build and retained logs", () => {
  const workflow = readFileSync(
    new URL("../../.github/workflows/local-auth.yml", import.meta.url),
    "utf8",
  )
  const step = (name) => {
    const start = workflow.indexOf(`      - name: ${name}\n`)
    if (start < 0) return ""
    const end = workflow.indexOf("\n      - ", start + 1)
    return workflow.slice(start, end < 0 ? undefined : end)
  }
  const load = step("Loaded paired-reader macOS acceptance")
  assert.ok(load.startsWith("      - name: Loaded paired-reader macOS acceptance"))
  assert.match(load, /if: runner\.os == 'macOS'/)
  assert.match(load, /timeout-minutes: 10/)
  const command = load.match(
    /run: node scripts\/cargo-test-parallel\/paired-reader-load\.mjs -- (.+)/,
  )?.[1]
  const build = workflow.match(
    /run: node scripts\/cargo-test-parallel\.mjs --concurrency 2 --partition \$\{\{ matrix.partition \}\} -- (.+)/,
  )?.[1]
  assert.ok(build)
  assert.equal(command, build)
  const upload = step("Retain loaded paired-reader macOS evidence")
  assert.match(upload, /if: \$\{\{ !cancelled\(\) && runner\.os == 'macOS' \}\}/)
  assert.match(upload, /path: target\/paired-reader-macos-load/)
  assert.match(upload, /if-no-files-found: error/)
})
