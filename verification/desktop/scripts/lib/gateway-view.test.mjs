/**
 * `mcp-apps-gateway.mjs`'s reading of the gateway's view, against the rows of
 * its design table (#384) that are decided here: which permission setup
 * answers and what it makes of the ended turn (A1–A6), which review is a
 * step's and when it has gone (R1–R5), and how long a step waits for that
 * review (R6–R8, #474). When each step takes its baseline is
 * the check's own, and is exercised only by running it.
 *
 * And `gateway-window.mjs`'s: what a text-only turn said, which its steps W2
 * and W3 compare the window's transcript with (#419), and the turns it cannot
 * compare.
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import { CannotRun } from "./cli.mjs"

import {
  admitOnce,
  appReviewWaitMs,
  callsOf,
  changedSamples,
  decideReviewWait,
  lastTurn,
  newReview,
  reviewAbsentMessage,
  reviewKeys,
  setupOutcome,
  stillPending,
  waitForAppReview,
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

describe("the app review a step waits for (#474)", () => {
  const opened = {
    executionId: "x",
    permissionId: "new",
    toolName: "app_delete_row",
    origin: { kind: "app", server: "mcptest", tool: "app_delete_row" },
  }
  const view = (permissions, transcriptState = "complete") => ({
    transcriptState,
    permissions,
  })
  /** A clock a test moves by `sleep`, so a review can be placed at a millisecond. */
  const clock = () => {
    let t = 0
    return {
      now: () => t,
      sleep: async (ms) => {
        t += ms
      },
    }
  }

  it("the bound is the client's call deadline", () => {
    assert.equal(appReviewWaitMs({ callDeadlineMs: 370_000 }), 370_000)
  })

  it("R6: a review listed while the call is still pending is the one to answer", () => {
    assert.deepEqual(
      decideReviewWait({ reviews: [opened], output: "pending" }, new Set()),
      { kind: "review", review: opened },
    )
  })

  it("R6: a review that arrives after 15s is taken when the deadline is the client's", async () => {
    const time = clock()
    const waited = await waitForAppReview({
      ...time,
      baseline: new Set(),
      deadlineMs: appReviewWaitMs({ callDeadlineMs: 370_000 }),
      pending: async () => "pending",
      read: async () => view(time.now() >= 16_000 ? [opened] : []),
    })
    assert.equal(waited.kind, "review")
    assert.equal(waited.review, opened)
    assert.ok(waited.samples.at(-1).ms >= 16_000)
    assert.ok(waited.samples.at(-1).ms < 370_000)
  })

  it("R6: the same review at 16s is missed when the deadline is 15s", async () => {
    const time = clock()
    const waited = await waitForAppReview({
      ...time,
      baseline: new Set(),
      deadlineMs: 15_000,
      pending: async () => "pending",
      read: async () => view(time.now() >= 16_000 ? [opened] : []),
    })
    assert.equal(waited.kind, "absent")
    assert.match(reviewAbsentMessage(waited.samples, waited.reads), /never listed/)
  })

  it("R7: the call leaving pending, with no new review, is answered", async () => {
    const time = clock()
    let reads = 0
    const waited = await waitForAppReview({
      ...time,
      baseline: new Set(),
      deadlineMs: 370_000,
      pending: async () => (reads > 1 ? "error: refused" : "pending"),
      read: async () => {
        reads += 1
        return view([])
      },
    })
    assert.deepEqual(
      { kind: waited.kind, output: waited.output },
      { kind: "answered", output: "error: refused" },
    )
  })

  it("R7: a review in the same read as an output that left pending is still the review", () => {
    assert.equal(
      decideReviewWait({ reviews: [opened], output: "error: refused" }, new Set()).kind,
      "review",
    )
  })

  it("R8: still pending at the deadline is absent, and the samples name the transcript", async () => {
    const time = clock()
    const waited = await waitForAppReview({
      ...time,
      baseline: new Set(),
      deadlineMs: 1_000,
      pending: async () => "pending",
      read: async () => view([], "partial"),
    })
    assert.equal(waited.kind, "absent")
    assert.ok(waited.samples.at(-1).ms >= 1_000)
    assert.equal(waited.reads > 1, true)
    const message = reviewAbsentMessage(waited.samples, waited.reads)
    assert.match(message, /never listed/)
    assert.match(message, /transcript stayed partial/)
    assert.match(message, /call output: pending/)
    assert.equal(changedSamples(waited.samples).length >= 1, true)
  })

  it("needs a deadline", async () => {
    await assert.rejects(
      () => waitForAppReview({ read: async () => view([]), baseline: new Set(), sleep: async () => {} }),
      /needs a deadline/,
    )
  })
})

describe("lastTurn", () => {
  const turn = (userText, parts) => ({ executionId: "e", userText, parts })
  const said =
    (parts, userText = "say it") =>
    () =>
      lastTurn({ messages: [turn(userText, parts)] })

  it("reads the last turn's text-only reply, its text parts joined and folded", () => {
    const view = {
      messages: [
        turn("first", [{ kind: "text", text: "old" }]),
        turn("say  it:\n", [
          { kind: "text", text: "Wab", messageId: "m1" },
          { kind: "text", text: "c12.\n", messageId: "m2" },
        ]),
      ],
    }
    assert.deepEqual(lastTurn(view), { user: "say it:", reply: "Wabc12." })
  })

  it("a reply with a tool or a local notice in it is not text-only: could not run", () => {
    for (const other of [
      { kind: "tool", toolId: "t" },
      { kind: "local_notice", text: "stopped" },
    ])
      assert.throws(
        said([{ kind: "text", text: "Wab" }, other, { kind: "text", text: "c12" }]),
        (error) => error instanceof CannotRun && /not text-only/.test(error.message),
      )
  })

  it("a thought part is left out, as the window does not draw it", () => {
    assert.deepEqual(
      said([
        { kind: "thought", text: "**Planning** `x`" },
        { kind: "text", text: "Wab" },
        { kind: "thought", text: "more" },
        { kind: "text", text: "c12" },
      ])(),
      { user: "say it", reply: "Wabc12" },
    )
    assert.throws(
      said([{ kind: "thought", text: "only a thought" }]),
      (error) => error instanceof CannotRun && /empty/.test(error.message),
    )
  })

  it("text the window draws otherwise — code, strong, anything not plain — could not run", () => {
    for (const text of ["`Wabc12`", "**Wabc12**", "Wabc12 *", "Wabc12 <b>"])
      assert.throws(
        said([{ kind: "text", text }]),
        (error) => error instanceof CannotRun && /not plain text/.test(error.message),
      )
    assert.throws(
      said([{ kind: "text", text: "Wabc12" }], "say `it`"),
      (error) => error instanceof CannotRun && /person's message/.test(error.message),
    )
  })

  it("an empty reply, a turn not yet answered, or no turn: could not run", () => {
    for (const parts of [[], [{ kind: "text", text: " \n" }]])
      assert.throws(said(parts), CannotRun)
    assert.throws(() => lastTurn({ messages: [] }), CannotRun)
  })
})
