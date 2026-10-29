import assert from "node:assert/strict"
import { test } from "node:test"
import { deliveryIdentity } from "./delivery.js"

test("delivery identities are immutable and canonicalize UUID case", () => {
  const identity = deliveryIdentity(
    "AAAAAAAA-1111-2222-3333-444444444444",
    "issue_comment",
  )
  assert.deepEqual(identity, {
    id: "aaaaaaaa-1111-2222-3333-444444444444",
    event: "issue_comment",
  })
  assert.equal(Object.isFrozen(identity), true)
})

test("missing, malformed and unbounded delivery headers are refused", () => {
  const id = "aaaaaaaa-1111-2222-3333-444444444444"
  for (const value of [null, "", "../../secret", id + "\n"]) {
    assert.equal(deliveryIdentity(value, "issues"), null)
  }
  for (const event of [null, "", "ISSUES", "a".repeat(65), "issues\n"]) {
    assert.equal(deliveryIdentity(id, event), null)
  }
})
