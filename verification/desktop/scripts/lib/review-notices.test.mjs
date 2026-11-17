import assert from "node:assert/strict"
import { describe, it } from "node:test"
import { reviewNoticeFailures } from "./review-notices.mjs"

describe("Settings notification permission evidence", () => {
  for (const [permission, rows, allowButtons, detail] of [
    ["default", 1, 1, "Off"],
    [
      "denied",
      1,
      0,
      "Blocked in this browser. Allow notifications for this site to turn them on.",
    ],
    ["granted", 1, 0, "On"],
    ["unsupported", 0, 0, ""],
  ])
    it(`accepts the ${permission} row and refuses contradictory button evidence`, () => {
      const actual = { permission, rows, allowButtons, detail }
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
        assert.notDeepEqual(reviewNoticeFailures({ ...actual, detail: "" }), [])
      }
    })

  it("does not infer an Allow button from the denied state's explanatory copy", () => {
    const actual = {
      permission: "denied",
      rows: 1,
      allowButtons: 0,
      detail:
        "Blocked in this browser. Allow notifications for this site to turn them on.",
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
          detail: "Allow",
        }).join(" "),
        /unexpected notification permission/,
      )
  })

  it("requires the status detail itself, not words in the row's label or control", () => {
    for (const detail of ["On", "Offline", "Off Allow", "", null, { text: "Off" }])
      assert.match(
        reviewNoticeFailures({
          permission: "default",
          rows: 1,
          allowButtons: 1,
          detail,
          text: "Review notifications Off Allow",
        }).join(" "),
        /does not explain its notification state/,
      )
    assert.notDeepEqual(
      reviewNoticeFailures({
        permission: "granted",
        rows: 1,
        allowButtons: 0,
        detail: "Off",
      }),
      [],
    )
    assert.notDeepEqual(
      reviewNoticeFailures({
        permission: "denied",
        rows: 1,
        allowButtons: 0,
        detail: "Blocked in this browser.",
      }),
      [],
    )
  })

  it("rejects contradictory control or detail evidence when the browser is unsupported", () => {
    for (const observed of [
      { allowButtons: 1, detail: "" },
      { allowButtons: 0, detail: "On" },
    ])
      assert.notDeepEqual(
        reviewNoticeFailures({ permission: "unsupported", rows: 0, ...observed }),
        [],
      )
  })
})
