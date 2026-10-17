import assert from "node:assert/strict"
import { describe, it } from "node:test"
import { reviewNoticeFailures } from "./review-notices.mjs"

describe("Settings notification permission evidence", () => {
  for (const [permission, rows, allowButtons, text] of [
    [
      "default",
      1,
      1,
      "Allow notifications to hear about a review while you are away from this window. Allow",
    ],
    [
      "denied",
      1,
      0,
      "Blocked in this browser. Allow notifications for this site to turn them on.",
    ],
    ["granted", 1, 0, "On. A review that is waiting shows a notification."],
    ["unsupported", 0, 0, ""],
  ])
    it(`accepts the ${permission} row and refuses contradictory button evidence`, () => {
      const actual = { permission, rows, allowButtons, text }
      assert.deepEqual(reviewNoticeFailures(actual), [])
      if (permission === "unsupported") {
        assert.notDeepEqual(reviewNoticeFailures({ ...actual, rows: 1 }), [])
      } else {
        assert.match(
          reviewNoticeFailures({
            ...actual,
            allowButtons: allowButtons === 0 ? 1 : 0,
          }).join(" "),
          /accessible Allow buttons/,
        )
        assert.notDeepEqual(reviewNoticeFailures({ ...actual, rows: 0 }), [])
        assert.notDeepEqual(reviewNoticeFailures({ ...actual, rows: 2 }), [])
        assert.notDeepEqual(reviewNoticeFailures({ ...actual, text: "" }), [])
      }
    })

  it("does not infer an Allow button from the denied state's explanatory copy", () => {
    const actual = {
      permission: "denied",
      rows: 1,
      allowButtons: 0,
      text: "Blocked in this browser. Allow notifications for this site to turn them on.",
    }
    assert.deepEqual(reviewNoticeFailures(actual), [])
    assert.match(
      reviewNoticeFailures({ ...actual, permission: "default" }).join(" "),
      /accessible Allow buttons/,
    )
  })

  it("rejects an unknown permission without indexing an inherited property", () => {
    for (const permission of ["unknown", "constructor", "toString", "__proto__"])
      assert.match(
        reviewNoticeFailures({
          permission,
          rows: 1,
          allowButtons: 1,
          text: "Allow",
        }).join(" "),
        /unexpected notification permission/,
      )
  })
})
