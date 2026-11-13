import assert from "node:assert/strict"
import { test } from "node:test"
import { withStepFailureEvidence } from "./stress-step-evidence.mjs"

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
