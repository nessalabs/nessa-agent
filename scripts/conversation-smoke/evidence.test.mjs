import assert from "node:assert/strict"
import { test } from "node:test"
import { evidenceFor, fixtureCorrelation, waitFor } from "./evidence.mjs"

test("fixture correlation requires one exact prompt marker", () => {
  assert.equal(
    fixtureCorrelation([
      { type: "text", text: "please continue\nfixtureCorrelation:execution-one" },
      { type: "image", data: "ignored" },
    ]),
    "execution-one",
  )
  assert.throws(() => fixtureCorrelation([{ type: "text", text: "no marker" }]))
  assert.throws(() =>
    fixtureCorrelation([
      { type: "text", text: "fixtureCorrelation:first\nfixtureCorrelation:second" },
    ]),
  )
})

test("provider evidence is keyed by session and expected execution", () => {
  const events = [
    { type: "prompt", providerSessionId: "session-a", expectedExecutionId: "one" },
    { type: "prompt", providerSessionId: "session-b", expectedExecutionId: "one" },
    { type: "steer", providerSessionId: "session-a", expectedExecutionId: "two" },
  ]
  assert.deepEqual(evidenceFor(events, "prompt", "session-a", "one"), [events[0]])
})

test("bounded waits poll an observed transition", async () => {
  let reads = 0
  const value = await waitFor(
    () => ++reads,
    (candidate) => candidate === 3,
    "third read",
    500,
  )
  assert.equal(value, 3)
})
