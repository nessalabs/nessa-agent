/**
 * `mcp-apps-gateway.mjs`'s reading of the gateway's view, one test at least
 * per row of its design table (#384): setup admits one call of the app tool,
 * and each step answers the review its own action opened.
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import {
  admitOnce,
  callsOf,
  newReview,
  reviewKeys,
  stillPending,
} from "./gateway-view.mjs"

const allow = { id: "allow_once", effect: "allow" }
const reject = { id: "reject_once", effect: "deny" }
const call = (toolId, tool, server = "mcptest") => ({
  executionId: "e",
  toolId,
  mcp: { server, tool },
})
const asks = (permissionId, toolId, kind = "harness", options = [allow, reject]) => ({
  executionId: "e",
  permissionId,
  toolId,
  origin: { kind },
  options,
})
const admit = (view, admitted = null, answered = new Set()) =>
  admitOnce(view, admitted, answered, "mcptest", "review_rows")

describe("admitOnce", () => {
  it("A1: allows the first call of the app tool, and names the call", () => {
    const view = {
      tools: [call("t1", "review_rows")],
      permissions: [asks("p1", "t1")],
    }
    const { allow: allowed, extra } = admit(view)
    assert.equal(allowed.permission.permissionId, "p1")
    assert.equal(allowed.option.id, "allow_once")
    assert.equal(allowed.call, "e:t1")
    assert.equal(extra, null)
  })

  it("A2: leaves a permission it already answered alone, and allows another of the same call", () => {
    const view = {
      tools: [call("t1", "review_rows")],
      permissions: [asks("p1", "t1"), asks("p2", "t1")],
    }
    const { allow: allowed, extra } = admit(view, "e:t1", new Set(["e:p1"]))
    assert.equal(allowed.permission.permissionId, "p2")
    assert.equal(extra, null)
    assert.deepEqual(admit(view, "e:t1", new Set(["e:p1", "e:p2"])), {
      allow: null,
      extra: null,
    })
  })

  it("A3: a second call of the app tool, once one is admitted, is not allowed but reported", () => {
    const view = {
      tools: [call("t1", "review_rows"), call("t2", "review_rows")],
      permissions: [asks("p1", "t1"), asks("p2", "t2")],
    }
    assert.deepEqual(admit(view, "e:t1", new Set(["e:p1"])), {
      allow: null,
      extra: "e:t2",
    })
  })

  it("A4: another tool's permission, an app's review, or one with no allow option is left alone", () => {
    const view = {
      tools: [
        call("t1", "app_delete_row"),
        call("t2", "review_rows", "other"),
        call("t3", "review_rows"),
        call("t4", "review_rows"),
      ],
      permissions: [
        asks("p1", "t1"),
        asks("p2", "t2"),
        asks("p3", "t3", "app"),
        asks("p4", "t4", "harness", [reject]),
      ],
    }
    assert.deepEqual(admit(view), { allow: null, extra: null })
  })
})

describe("callsOf", () => {
  it("A5: counts distinct calls of the tool, however often the view lists each", () => {
    const view = {
      tools: [
        call("t1", "review_rows"),
        call("t1", "review_rows"),
        call("t2", "review_rows"),
        call("t3", "show_chart"),
      ],
    }
    assert.deepEqual(
      callsOf(view, "mcptest", "review_rows").map((each) => each.toolId),
      ["t1", "t2"],
    )
  })

  it("A6: one call is one", () => {
    const view = { tools: [call("t1", "review_rows"), call("t1", "review_rows")] }
    assert.equal(callsOf(view, "mcptest", "review_rows").length, 1)
  })
})

describe("newReview", () => {
  const review = (permissionId) => ({ executionId: "x", permissionId })

  it("R1–R3: picks the review not in the baseline taken before the action", () => {
    const stale = review("old")
    const opened = review("new")
    assert.equal(newReview([stale, opened], reviewKeys([stale])), opened)
  })

  it("R4: a stale review, pending before the action, is never picked", () => {
    const stale = review("old")
    assert.equal(newReview([stale], reviewKeys([stale])), undefined)
  })

  it("R5: no review at all is none", () => {
    assert.equal(newReview([], new Set()), undefined)
  })

  it("the review is gone when its own key is, whatever else is pending", () => {
    const stale = review("old")
    const opened = review("new")
    assert.equal(stillPending([stale, opened], opened), true)
    assert.equal(stillPending([stale], opened), false)
  })
})
