import assert from "node:assert/strict"
import { test } from "node:test"
import { modeIndependentContent } from "./mode-content.mjs"

const view = {
  revision: "r1",
  approvalMode: "ask",
  serverNowMs: 1000,
  conversationId: "conversation",
  messages: [],
  pending: [],
  permissions: [],
  questions: [],
  artifacts: [],
  tools: [],
  lifecycle: { phase: "attached" },
  capabilities: { permissions: true },
}

test("mode and observation metadata do not imply semantic output", () => {
  assert.equal(
    modeIndependentContent(view),
    modeIndependentContent({
      ...view,
      revision: "r2",
      approvalMode: "auto",
      serverNowMs: 2000,
    }),
  )
})

test("every remaining field participates, including future published fields", () => {
  for (const key of Object.keys(view).filter(
    (key) => !["revision", "approvalMode", "serverNowMs"].includes(key),
  )) {
    assert.notEqual(
      modeIndependentContent(view),
      modeIndependentContent({ ...view, [key]: null }),
      key,
    )
  }
  assert.notEqual(
    modeIndependentContent(view),
    modeIndependentContent({ ...view, futureField: "changed" }),
  )
})

test("content comparison preserves its input", () => {
  const snapshot = structuredClone(view)
  modeIndependentContent(Object.freeze(view))
  assert.deepEqual(view, snapshot)
})
