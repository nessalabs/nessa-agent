import assert from "node:assert/strict"
import test from "node:test"
import { runScenario, scenarios, validate } from "./check-container.mjs"

function report(scenario) {
  return {
    test: scenario.test,
    timed_out: false,
    supervisor_pid: scenario.init ? 7 : 1,
    supervisor_ppid: scenario.init ? 1 : 0,
    test_exit: scenario.init ? 0 : 101,
    output: scenario.init
      ? "running 1 test\ntest result: ok. 1 passed; 0 failed; 0 ignored;"
      : "running 1 test\nCleanupUncertain\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;",
    new_orphans: scenario.init ? [] : [{ pid: 10, ppid: 1, pgid: 9, state: "Z" }],
    retained_directories: scenario.init ? [] : ["/tmp/nessa-agent-owned"],
  }
}

for (const scenario of scenarios) {
  test(`${scenario.init ? "init" : "negative"} ${scenario.test}`, () => {
    assert.equal(validate(report(scenario), scenario).test, scenario.test)
  })
}

test("zero selected tests cannot pass", () => {
  const positive = scenarios[1]
  assert.throws(
    () =>
      validate(
        {
          ...report(positive),
          output: "running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored;",
        },
        positive,
      ),
    /one test/,
  )
})
test("a different negative failure is a harness failure", () => {
  assert.throws(
    () =>
      validate(
        {
          ...report(scenarios[0]),
          output: report(scenarios[0]).output.replace("CleanupUncertain", "Deadline"),
        },
        scenarios[0],
      ),
    /CleanupUncertain/,
  )
})
test("the negative requires a new adopted zombie", () => {
  for (const new_orphans of [
    [],
    [{ pid: 10, ppid: 2, pgid: 9, state: "Z" }],
    [{ pid: 10, ppid: 1, pgid: 9, state: "S" }],
  ]) {
    assert.throws(
      () => validate({ ...report(scenarios[0]), new_orphans }, scenarios[0]),
      /zombie/,
    )
  }
})
test("directory retention is required in the negative directory case", () => {
  assert.throws(
    () => validate({ ...report(scenarios[0]), retained_directories: [] }, scenarios[0]),
    /released/,
  )
})
test("positive failures, zombies, retention and timeout cannot pass", () => {
  const positive = scenarios[1]
  for (const change of [
    { test_exit: 101 },
    { new_orphans: report(scenarios[0]).new_orphans },
    { retained_directories: ["/tmp/nessa-agent-owned"] },
    { timed_out: true },
  ]) {
    assert.throws(
      () => validate({ ...report(positive), ...change }, positive),
      /Harness failure/,
    )
  }
})

const options = {
  binary: "/binary",
  manifest: "/compiled/crate",
  fixture: "/compiled/crate/tests/infrastructure/acp/contracts/fixtures",
  image: "acceptance",
}

test("four scenarios use distinct containers, same inputs and readonly mounts", async () => {
  const creates = [],
    removes = []
  for (const scenario of scenarios) {
    await runScenario(options, scenario, async (args) => {
      if (args[0] === "create") creates.push(args)
      if (args[0] === "rm") removes.push(args)
      return args[0] === "start" ? JSON.stringify(report(scenario)) : ""
    })
  }
  assert.equal(new Set(creates.map((args) => args[2])).size, 4)
  assert.equal(removes.length, 4)
  for (const args of creates) {
    assert.ok(args.includes("--network=none"))
    assert.ok(args.includes("--read-only"))
    assert.ok(args.includes("type=bind,src=/binary,dst=/probe/nessa-sdk-tests,readonly"))
    assert.ok(
      args.includes(`type=bind,src=${options.fixture},dst=${options.fixture},readonly`),
    )
    assert.equal(args.at(-2), options.image)
  }
  assert.equal(creates.filter((args) => args.includes("--init")).length, 2)
})

for (const stage of ["create", "start", "validation", "interrupt"]) {
  test(`removal is attempted after ${stage} failure`, async () => {
    const calls = [],
      controller = new AbortController()
    const run = async (args, operation) => {
      calls.push([args, operation])
      if (args[0] === stage) throw new Error("injected docker failure")
      if (args[0] === "start" && stage === "interrupt") {
        controller.abort()
        throw new Error("interrupted")
      }
      if (args[0] === "start")
        return JSON.stringify(
          stage === "validation"
            ? { ...report(scenarios[0]), timed_out: true }
            : report(scenarios[0]),
        )
      return ""
    }
    await assert.rejects(
      runScenario({ ...options, signal: controller.signal }, scenarios[0], run),
    )
    assert.equal(calls.at(-1)[0][0], "rm")
    assert.equal(calls.at(-1)[1].signal, undefined)
    assert.equal(calls.at(-1)[1].timeout, 10000)
  })
}

test("supervisor ownership and selected test must agree with scenario", () => {
  for (const [scenario, change] of [
    [scenarios[0], { test: "another-test" }],
    [scenarios[0], { supervisor_pid: 7 }],
    [scenarios[1], { supervisor_pid: 1 }],
    [scenarios[1], { supervisor_ppid: 7 }],
    [scenarios[0], { test_exit: 0 }],
  ])
    assert.throws(
      () => validate({ ...report(scenario), ...change }, scenario),
      /Harness failure/,
    )
})

test("a failed removal prevents acceptance", async () => {
  await assert.rejects(
    runScenario(options, scenarios[0], async (args) => {
      if (args[0] === "rm") throw new Error("removal failed")
      return args[0] === "start" ? JSON.stringify(report(scenarios[0])) : ""
    }),
    /removal failed/,
  )
})
