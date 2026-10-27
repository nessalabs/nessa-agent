import { test } from "node:test"
import assert from "node:assert/strict"
import { messageText, missingVmRefused, settledMessage } from "./evidence.mjs"

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

test("text evidence joins streaming deltas before matching", () => {
  assert.equal(
    messageText({
      parts: [
        { kind: "text", text: "NESSA_" },
        { kind: "tool", text: "ignore" },
        { kind: "text", text: "DELEGATED_OK" },
      ],
    }),
    "NESSA_DELEGATED_OK",
  )
})

test("missing VM evidence accepts only environment refusal or retained cleanup failure", () => {
  assert.equal(missingVmRefused("environment_unavailable", { phase: "failed" }), true)
  assert.equal(
    missingVmRefused("agent_operation_failed", {
      phase: "failed",
      failure: { code: "cleanup" },
    }),
    true,
  )
  for (const code of [
    "agent_operation_failed",
    "conversation_closed",
    "invalid_request",
    undefined,
  ]) {
    assert.equal(
      missingVmRefused(code, { phase: "failed", failure: { code: "provider" } }),
      false,
    )
  }
  assert.equal(
    missingVmRefused("agent_operation_failed", {
      phase: "attached",
      failure: { code: "cleanup" },
    }),
    false,
  )
})
