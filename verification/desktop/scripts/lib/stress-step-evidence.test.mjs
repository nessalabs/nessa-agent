import assert from "node:assert/strict"
import { test } from "node:test"
import { CannotRun, report } from "./cli.mjs"
import { reportStressStep, withStepFailureEvidence } from "./stress-step-evidence.mjs"

const withoutDiagnostics = async (body) => {
  const write = process.stderr.write
  process.stderr.write = () => true
  try {
    return await body()
  } finally {
    process.stderr.write = write
  }
}

test("an assertion failure is captured before navigation", async () => {
  const events = [],
    failures = []
  await withStepFailureEvidence(
    async () => {
      events.push("fixture")
      failures.push("wrong app mode")
    },
    {
      failures,
      capture: async () => events.push("capture"),
      cleanup: async () => events.push("main"),
    },
  )
  assert.deepEqual(events, ["fixture", "capture", "main"])
  assert.deepEqual(failures, ["wrong app mode"])
})

test("a thrown browser error is captured before navigation and remains the error", async () => {
  const events = [],
    original = new Error("locator timeout")
  await assert.rejects(
    withStepFailureEvidence(
      async () => {
        events.push("fixture")
        throw original
      },
      {
        failures: [],
        capture: async () => events.push("capture"),
        cleanup: async () => events.push("main"),
      },
    ),
    (error) => error === original,
  )
  assert.deepEqual(events, ["fixture", "capture", "main"])
})

test("a broken screenshot preserves the original browser error and still cleans up", async () => {
  const events = [],
    original = new Error("page went away")
  await assert.rejects(
    withStepFailureEvidence(
      async () => {
        throw original
      },
      {
        failures: [],
        capture: async () => {
          events.push("capture")
          throw new Error("screenshot failed")
        },
        cleanup: async () => events.push("main"),
      },
    ),
    (error) => error === original,
  )
  assert.deepEqual(events, ["capture", "main"])
})

test("a successful step returns its result and cleans up without a failure shot", async () => {
  const events = []
  assert.equal(
    await withStepFailureEvidence(async () => 42, {
      failures: [],
      capture: async () => events.push("capture"),
      cleanup: async () => events.push("main"),
    }),
    42,
  )
  assert.deepEqual(events, ["main"])
})

test("an action and cleanup failure retain both identities after capture", async () => {
  const original = new Error("browser original"),
    cleanup = new Error("navigation cleanup"),
    events = []
  await assert.rejects(
    withStepFailureEvidence(
      async () => {
        throw original
      },
      {
        failures: [],
        capture: async () => events.push("capture"),
        cleanup: async () => {
          events.push("cleanup")
          throw cleanup
        },
      },
    ),
    (error) => {
      assert.ok(error instanceof AggregateError)
      assert.deepEqual(error.errors, [original, cleanup])
      assert.equal(error.cause, original)
      return true
    },
  )
  assert.deepEqual(events, ["capture", "cleanup"])
})

test("a cleanup-only error retains its identity without a false action failure", async () => {
  const cleanup = new Error("cleanup"),
    events = []
  await assert.rejects(
    withStepFailureEvidence(async () => 42, {
      failures: [],
      capture: async () => events.push("capture"),
      cleanup: async () => {
        events.push("cleanup")
        throw cleanup
      },
    }),
    (error) => error === cleanup,
  )
  assert.deepEqual(events, ["cleanup"])
})

test("falsy action exceptions remain failures in actual CLI reports", async () => {
  await withoutDiagnostics(async () => {
    for (const value of [
      undefined,
      null,
      false,
      0,
      "",
      { message: "" },
      Object.create(null),
    ]) {
      const rep = report("stress", {})
      let calls = 0
      const step = { kind: "huddle", index: 1 }
      const entry = await reportStressStep(
        rep,
        { name: "001 huddle" },
        step,
        async () => {
          calls += 1
          throw value
        },
      )
      assert.equal(calls, 1)
      assert.equal(entry.ok, false)
      assert.equal(entry.cannotRun, false)
      assert.ok(entry.error.length > 0)
      assert.equal(entry.seen, step)
    }
  })
})

test("an action and cleanup exception both survive the actual CLI report", async () => {
  await withoutDiagnostics(async () => {
    const rep = report("stress", {})
    const entry = await reportStressStep(rep, { name: "huddle" }, {}, (failures) =>
      withStepFailureEvidence(
        async () => {
          throw new Error("browser original")
        },
        {
          failures,
          capture: async () => {},
          cleanup: async () => {
            throw new Error("navigation cleanup")
          },
        },
      ),
    )
    assert.equal(entry.ok, false)
    assert.match(entry.error, /browser original/)
    assert.match(entry.error, /navigation cleanup/)
  })
})

test("step reporting retains an RPC failure's typed code and immediate cause", async () => {
  await withoutDiagnostics(async () => {
    const fault = Object.assign(new Error("request refused"), {
      code: "not_authenticated",
      cause: { code: "connection_closed" },
    })
    const entry = await reportStressStep(
      report("stress", {}),
      { name: "huddle" },
      {},
      async () => {
        throw fault
      },
    )
    assert.equal(entry.ok, false)
    assert.match(entry.error, /request refused/)
    assert.match(entry.error, /not_authenticated/)
    assert.match(entry.error, /connection_closed/)
  })
})

test("step reporting retains assertions and the CLI's typed could-not-run classification", async () => {
  await withoutDiagnostics(async () => {
    const rep = report("stress", {})
    const success = await reportStressStep(rep, { name: "success" }, {}, async () => {})
    assert.equal(success.ok, true)
    const assertion = await reportStressStep(
      rep,
      { name: "assertion" },
      {},
      async (failures) => {
        failures.push("wrong app mode")
        throw new Error("browser failed")
      },
    )
    assert.deepEqual(assertion.failures, ["wrong app mode"])
    assert.equal(assertion.ok, false)
    const unavailable = await reportStressStep(
      rep,
      { name: "unavailable" },
      {},
      async () => {
        throw new CannotRun("no window")
      },
    )
    assert.equal(unavailable.cannotRun, true)
    assert.equal(unavailable.ok, false)
  })
})
