/**
 * `mcp-apps-gateway.mjs`'s reading of the gateway's view, against the rows of
 * its design table (#384) that are decided here: which permission setup
 * answers and what it makes of the ended turn (A1–A6), and which review is a
 * step's and when it has gone (R1–R5). When each step takes its baseline is
 * the check's own, and is exercised only by running it.
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import {
  admitOnce,
  callsOf,
  newReview,
  reviewKeys,
  setupOutcome,
  stillPending,
  setupTurnEnded,
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

  const twice = {
    tools: [call("t1", "review_rows")],
    permissions: [asks("p1", "t1"), asks("p2", "t1")],
  }

  it("A2: leaves a permission it already answered alone", () => {
    assert.deepEqual(admit(twice, "e:t1", new Set(["e:p1", "e:p2"])), {
      allow: null,
      extra: null,
    })
  })

  it("A2b: allows another permission of the admitted call: it is the same one call", () => {
    const { allow: allowed, extra } = admit(twice, "e:t1", new Set(["e:p1"]))
    assert.equal(allowed.permission.permissionId, "p2")
    assert.equal(allowed.call, "e:t1")
    assert.equal(extra, null)
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

  it("keeps each call as the view first lists it", () => {
    const view = {
      tools: [
        { ...call("t1", "review_rows"), status: "completed" },
        { ...call("t1", "review_rows"), status: "running" },
      ],
    }
    assert.equal(callsOf(view, "mcptest", "review_rows")[0].status, "completed")
  })
})

describe("setupOutcome", () => {
  const done = (toolId, extra = {}) => ({
    ...call(toolId, "review_rows"),
    status: "completed",
    ...extra,
    mcp: { server: "mcptest", tool: "review_rows", resourceUri: "ui://r", ...extra.mcp },
  })
  const ended = (tools, status = "completed") => ({ tools, messages: [{ status }] })
  const outcome = (view) => setupOutcome(view, "mcptest", "review_rows")

  it("A5: more than one distinct call is repeated, whatever their state", () => {
    const { kind, calls } = outcome(ended([done("t1"), done("t1"), done("t2")]))
    assert.equal(kind, "repeated")
    assert.deepEqual(
      calls.map((each) => each.toolId),
      ["t1", "t2"],
    )
  })

  it("A6: one completed call naming its resourceUri, in a completed turn, is ready", () => {
    const view = ended([done("t1"), done("t1")])
    assert.deepEqual(outcome(view), { kind: "ready", call: view.tools[0] })
  })

  it("A6: anything less is unusable — no call, a failed call, no resourceUri, a failed turn", () => {
    for (const view of [
      ended([]),
      ended([done("t1", { status: "failed" })]),
      ended([done("t1", { mcp: { resourceUri: undefined } })]),
      ended([done("t1")], "failed"),
    ])
      assert.equal(outcome(view).kind, "unusable")
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

describe("setupTurnEnded", () => {
  it("a turn running, queued, read as unresolved (#448), or in any other status has not ended", () => {
    for (const status of [
      "running",
      "queued",
      "unresolved",
      "injected",
      "a-future-status",
    ])
      assert.equal(setupTurnEnded(status), false)
  })
  it("a completed, failed or cancelled turn has", () => {
    for (const status of ["completed", "failed", "cancelled"])
      assert.equal(setupTurnEnded(status), true)
  })
})
