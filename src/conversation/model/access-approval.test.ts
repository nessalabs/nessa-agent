import { describe, expect, it } from "vitest"
import {
  stepApproval,
  type AccessApproval,
  type ApprovalSnapshot,
} from "./access-approval"

/** One row of the table in `access-approval.ts` each. */
const ask = { askId: "a" }
type Approval = AccessApproval<typeof ask>
const look = (facts: Partial<ApprovalSnapshot> = {}): ApprovalSnapshot => ({
  askId: "a",
  runsAt: "ask",
  underway: undefined,
  working: false,
  revision: 1,
  ...facts,
})
const raising: Approval = { state: "raising", ask, mode: "full" }
const waiting: Approval = { state: "waiting", ask, mode: "full", takenAt: 1 }
const due: Approval = { state: "due", ask, mode: "full" }
const answering: Approval = { state: "answering", ask, mode: "full" }
const approved: Approval = { state: "approved", ask, mode: "full" }
const ended: Approval = { state: "ended", ask, mode: "full", ending: "refused" }
const snapshot = (approval: Approval, facts: Partial<ApprovalSnapshot> = {}) =>
  stepApproval(approval, { kind: "snapshot", snapshot: look(facts) })

describe("raising", () => {
  it("waits at the raise's view once taken or held for the turn", () => {
    for (const outcome of ["taken", "pending"] as const)
      expect(
        stepApproval(raising, {
          kind: "raised",
          outcome,
          snapshot: look({ revision: 4 }),
        }),
      ).toEqual({ ...waiting, takenAt: 4 })
  })
  it("judges the look it landed in: an access already there sends at once", () => {
    expect(
      stepApproval(raising, {
        kind: "raised",
        outcome: "taken",
        snapshot: look({ runsAt: "full", revision: 2 }),
      }),
    ).toEqual(due)
    // Held for a turn's end: waits through it.
    expect(
      stepApproval(raising, {
        kind: "raised",
        outcome: "pending",
        snapshot: look({ runsAt: "ask", underway: "full", working: true, revision: 3 }),
      }),
    ).toEqual({ ...waiting, takenAt: 3 })
  })
  it("ends, saying which, when another change is on its way or it was refused", () => {
    for (const outcome of ["changing", "refused"] as const)
      expect(
        stepApproval(raising, { kind: "raised", outcome, snapshot: look() }),
      ).toEqual({
        state: "ended",
        ask,
        mode: "full",
        ending: outcome,
      })
  })
  it("goes, sending nothing, when the ask left", () => {
    expect(snapshot(raising, { askId: undefined })).toBeUndefined()
    expect(
      stepApproval(raising, {
        kind: "raised",
        outcome: "taken",
        snapshot: look({ askId: "b" }),
      }),
    ).toBeUndefined()
  })
})

describe("waiting", () => {
  it("goes, sending nothing, when the ask left, even as the access arrives", () => {
    expect(snapshot(waiting, { askId: undefined, runsAt: "full" })).toBeUndefined()
    expect(snapshot(waiting, { askId: "b", runsAt: "full" })).toBeUndefined()
  })
  it("waits for a change on its way to this access, and through a turn at work", () => {
    expect(snapshot(waiting, { runsAt: undefined, underway: "full" })).toBe(waiting)
    expect(snapshot(waiting, { runsAt: "full", working: true })).toBe(waiting)
  })
  it("ends when a change on its way is to another access", () => {
    expect(snapshot(waiting, { underway: "auto" })).toMatchObject({
      state: "ended",
      ending: "changed-elsewhere",
    })
  })
  it("comes due once the chat runs at the access or more", () => {
    expect(snapshot(waiting, { runsAt: "full", revision: 2 })).toEqual(due)
    const forAuto: Approval = { ...waiting, mode: "auto" }
    expect(snapshot(forAuto, { runsAt: "full", revision: 2 })).toEqual({
      state: "due",
      ask,
      mode: "auto",
    })
  })
  it("waits while the access is not known and no change is named", () => {
    expect(snapshot(waiting, { runsAt: undefined, revision: 5 })).toBe(waiting)
  })
  it("ends when a change on its way is to less, but waits for one to more", () => {
    const forAuto: Approval = { ...waiting, mode: "auto" }
    expect(snapshot(forAuto, { underway: "full" })).toBe(forAuto)
  })
  it("waits through the raise's own view, and ends on a newer one elsewhere", () => {
    expect(snapshot(waiting, { runsAt: "ask", revision: 1 })).toBe(waiting)
    expect(snapshot(waiting, { runsAt: "ask", revision: 2 })).toMatchObject({
      state: "ended",
      ending: "changed-elsewhere",
    })
  })
})

describe("due", () => {
  it("goes, sending nothing, when the ask leaves before a card starts the answer", () => {
    expect(snapshot(due, { askId: undefined, runsAt: "full" })).toBeUndefined()
    expect(snapshot(due, { askId: "b", runsAt: "full" })).toBeUndefined()
  })
  it("stays due on any other look, and answers once a card starts it", () => {
    expect(snapshot(due, { runsAt: "ask", revision: 9 })).toBe(due)
    expect(stepApproval(due, { kind: "answering" })).toEqual(answering)
  })
  it("takes no answer it never started", () => {
    expect(stepApproval(due, { kind: "answered", sent: true })).toBe(due)
  })
})

describe("answering", () => {
  it("keeps on whatever a look says: the ask leaving is its own answer", () => {
    expect(snapshot(answering, { askId: undefined, runsAt: "ask" })).toBe(answering)
  })
  it("is approved once sent, and ends when not", () => {
    expect(stepApproval(answering, { kind: "answered", sent: true })).toEqual(approved)
    expect(stepApproval(answering, { kind: "answered", sent: false })).toMatchObject({
      state: "ended",
      ending: "not-sent",
    })
  })
})

describe("approved", () => {
  it("stays while the ask leaves and the chat runs at the access", () => {
    expect(snapshot(approved, { askId: undefined, runsAt: "full" })).toBe(approved)
  })
  it("goes for another ask, or another access", () => {
    expect(snapshot(approved, { askId: "b", runsAt: "full" })).toBeUndefined()
    expect(snapshot(approved, { askId: undefined, runsAt: "ask" })).toBeUndefined()
  })
})

describe("ended", () => {
  it("goes with its ask, and starts again on Approve", () => {
    expect(snapshot(ended, { askId: undefined })).toBeUndefined()
    expect(stepApproval(ended, { kind: "approve", ask, mode: "full" })).toEqual(raising)
  })
  it("a second Approve while one is under way changes nothing", () => {
    expect(stepApproval(waiting, { kind: "approve", ask, mode: "full" })).toBe(waiting)
  })
})
