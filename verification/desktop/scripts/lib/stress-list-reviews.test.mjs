import assert from "node:assert/strict"
import { test } from "node:test"
import { requireCompactReview, requirePreviewReview } from "./stress-list-reviews.mjs"

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
