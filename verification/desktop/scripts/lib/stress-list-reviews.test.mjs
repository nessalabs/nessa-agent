import assert from "node:assert/strict"
import { test } from "node:test"
import {
  requireCompactReview,
  requirePreviewReview,
  requireListTurnResult,
} from "./stress-list-reviews.mjs"

const compact = {
  whole: true,
  command: "$ node --version",
  grants: 1,
  refusals: 1,
  reviewInChat: 0,
}
const preview = {
  whole: false,
  command: "other …",
  grants: 0,
  refusals: 1,
  reviewInChat: 1,
}
test("compact approval and preview-only review are distinct contracts", () => {
  requireCompactReview(compact)
  requirePreviewReview(preview)
  assert.throws(() => requireCompactReview(preview))
  assert.throws(() => requirePreviewReview(compact))
})
test("a leaked preview grant, missing chat route or false whole marker fails", () => {
  assert.throws(() => requirePreviewReview({ ...preview, grants: 1 }), /allow action/)
  assert.throws(
    () => requirePreviewReview({ ...preview, reviewInChat: 0 }),
    /Review in Chat/,
  )
  assert.throws(() => requirePreviewReview({ ...preview, whole: true }), /preview/)
})
test("a missing compact grant or a substituted command cannot pass", () => {
  assert.throws(() => requireCompactReview({ ...compact, grants: 0 }), /Allow once/)
  assert.throws(() => requireCompactReview({ ...compact, command: "$ other" }))
  assert.throws(() => requireCompactReview({ ...compact, refusals: 0 }), /Deny once/)
})

const permission = { executionId: "current", toolId: "original" }
const result = {
  permission,
  answer: "allow",
  expectedReply: "Allowed. unique request",
  expectedVersion: "v24.0.0",
  turn: {
    executionId: "current",
    status: "completed",
    parts: [{ kind: "text", text: "Allowed. unique request" }],
  },
  tools: [
    {
      executionId: "current",
      toolId: "original",
      status: "completed",
      details: "v24.0.0",
    },
  ],
}
test("only the current turn's exact reply and original tool completion satisfy an answer", () => {
  requireListTurnResult(result)
  assert.throws(
    () =>
      requireListTurnResult({
        ...result,
        turn: {
          ...result.turn,
          parts: [{ kind: "text", text: "Allowed. earlier request" }],
        },
      }),
    /wrong answer/,
  )
  assert.throws(
    () =>
      requireListTurnResult({
        ...result,
        turn: { ...result.turn, executionId: "earlier" },
      }),
    /different turn/,
  )
  assert.throws(
    () =>
      requireListTurnResult({
        ...result,
        tools: [{ ...result.tools[0], toolId: "another" }],
      }),
    /original review/,
  )
  assert.throws(
    () =>
      requireListTurnResult({
        ...result,
        tools: [{ ...result.tools[0], details: "fabricated" }],
      }),
    /actual runtime/,
  )
  assert.throws(() =>
    requireListTurnResult({
      ...result,
      tools: [{ ...result.tools[0], status: "failed" }],
    }),
  )
})
test("a denied current turn must have failed its original tool without executable output", () => {
  const denied = {
    ...result,
    answer: "deny",
    expectedReply: "Denied. unique request",
    turn: { ...result.turn, parts: [{ kind: "text", text: "Denied. unique request" }] },
    tools: [{ ...result.tools[0], status: "failed", details: "" }],
  }
  requireListTurnResult(denied)
  assert.throws(
    () =>
      requireListTurnResult({
        ...denied,
        tools: [{ ...denied.tools[0], details: "v24.0.0" }],
      }),
    /executable output/,
  )
  assert.throws(() => requireListTurnResult({ ...denied, tools: result.tools }))
})
