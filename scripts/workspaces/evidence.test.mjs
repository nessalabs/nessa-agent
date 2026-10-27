import { test } from "node:test"
import assert from "node:assert/strict"
import { settledMessage } from "./evidence.mjs"

test("waits for the admitted execution rather than accepting a previous completion", () => {
  const old = { executionId: "previous", status: "completed" }
  const pending = { executionId: "new", status: "running" }
  assert.equal(settledMessage({ messages: [old] }, "new"), null)
  assert.equal(settledMessage({ messages: [old, pending] }, "new"), null)
  const completed = { ...pending, status: "completed" }
  assert.equal(settledMessage({ messages: [old, completed] }, "new"), completed)
  for (const status of ["failed", "cancelled"]) {
    const terminal = { ...pending, status }
    assert.equal(settledMessage({ messages: [old, terminal] }, "new"), terminal)
  }
})
