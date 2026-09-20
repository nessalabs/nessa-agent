import { describe, expect, it } from "vitest"

import {
  applySetupHandoff,
  createHandoffGate,
  handoffRejection,
  noSetupHandoff,
  shouldHandOver,
  type SetupHandoffState,
} from "./setup-handoff"
import { setupRecovery } from "./setup-recovery"
import type { SetupHandoff } from "../../host"

/** Every event in order, the way the surface would record them. */
function record(...events: Parameters<typeof applySetupHandoff>[1][]): SetupHandoffState {
  return events.reduce(applySetupHandoff, noSetupHandoff)
}

/** An answer to whichever attempt is outstanding, which is the ordinary case. */
function answer(state: SetupHandoffState, outcome: SetupHandoff): SetupHandoffState {
  expect(state.requested, "no attempt is outstanding").toBeDefined()
  return applySetupHandoff(state, { type: "settled", attempt: state.requested!, outcome })
}

describe("when the handoff is asked for", () => {
  it("waits for setup to be over", () => {
    expect(shouldHandOver(noSetupHandoff, true)).toBe(false)
    expect(shouldHandOver(noSetupHandoff, false)).toBe(true)
  })

  it("asks once, however many times the surface is drawn", () => {
    const asked = record({ type: "requested" })
    expect(shouldHandOver(asked, false)).toBe(false)
    // The same event again is the same state, so nothing re-renders on it.
    expect(applySetupHandoff(asked, { type: "requested" })).toBe(asked)
  })

  it("asks again after a retry, and not before", () => {
    const failed = record(
      { type: "requested" },
      { type: "settled", attempt: 1, outcome: handoffRejection(new Error("no panel")) },
    )
    expect(shouldHandOver(failed, false)).toBe(false)
    expect(shouldHandOver(applySetupHandoff(failed, { type: "retry" }), false)).toBe(true)
  })
})

describe("what the handoff leaves on screen", () => {
  it("shows nothing while the answer is outstanding", () => {
    expect(setupRecovery(record({ type: "requested" }).outcome)).toBeNull()
  })

  it("shows nothing when this window is on its way out", () => {
    const handedOver = record(
      { type: "requested" },
      { type: "settled", attempt: 1, outcome: { outcome: "handed-over" } },
    )
    expect(setupRecovery(handedOver.outcome)).toBeNull()
  })

  it("keeps the cause of a rejected handoff rather than a bare failure", () => {
    const cause = new Error("there is no panel to summon")
    const rejected = record(
      { type: "requested" },
      { type: "settled", attempt: 1, outcome: handoffRejection(cause) },
    )
    expect(rejected.outcome).toEqual({ outcome: "panel-unavailable", cause })
    expect(setupRecovery(rejected.outcome)?.offer).toBe("hand-over-again")
  })

  it("ignores an answer belonging to an attempt a retry abandoned", () => {
    // A retry is only offered once an answer is on screen, so nothing is in
    // flight when one is made — this is what keeps that from being load-bearing.
    // `finishSetupWindow` cannot be cancelled, so a call that does outlive its
    // attempt still names the attempt it was made for.
    const second = record(
      { type: "requested" },
      { type: "settled", attempt: 1, outcome: handoffRejection(new Error("first")) },
      { type: "retry" },
      { type: "requested" },
    )
    expect(second.requested).toBe(2)

    const late = applySetupHandoff(second, {
      type: "settled",
      attempt: 1,
      outcome: handoffRejection(new Error("late")),
    })
    // Unchanged: the second attempt is still outstanding and nothing is on screen.
    expect(late).toBe(second)
    expect(setupRecovery(late.outcome)).toBeNull()

    // The attempt actually waited on still lands.
    const settled = answer(late, { outcome: "handed-over" })
    expect(settled.outcome).toEqual({ outcome: "handed-over" })
    expect(settled.requested).toBeUndefined()
  })
})

describe("closing the window instead", () => {
  it("says so only when the window system refused", () => {
    const asked = record({ type: "requested" })
    expect(
      applySetupHandoff(asked, { type: "closed", close: { outcome: "closed" } })
        .closeFailed,
    ).toBe(false)
    // A browser has no window of its own to close. That is not a failure.
    expect(
      applySetupHandoff(asked, { type: "closed", close: { outcome: "no-native-host" } })
        .closeFailed,
    ).toBe(false)
    expect(
      applySetupHandoff(asked, {
        type: "closed",
        close: { outcome: "close-failed", cause: "the window server said no" },
      }).closeFailed,
    ).toBe(true)
  })

  it("keeps the screen it is shown on", () => {
    const refused = record(
      { type: "requested" },
      { type: "settled", attempt: 1, outcome: handoffRejection(new Error("no panel")) },
      { type: "closed", close: { outcome: "close-failed", cause: "no" } },
    )
    // The note under the buttons changed; the screen and its ways out did not.
    expect(refused.closeFailed).toBe(true)
    expect(setupRecovery(refused.outcome)?.heading).toBe("Nessa could not open the panel")
  })

  it("stops saying the close failed once the handoff is asked for again", () => {
    const retried = record(
      { type: "requested" },
      { type: "settled", attempt: 1, outcome: handoffRejection(new Error("no panel")) },
      { type: "closed", close: { outcome: "close-failed", cause: "no" } },
      { type: "retry" },
    )
    // Everything the failed attempt left is gone, except the count — the next
    // attempt needs a number the abandoned one cannot answer to.
    expect(retried).toEqual({ ...noSetupHandoff, attempts: 1 })
    expect(retried.closeFailed).toBe(false)
    expect(setupRecovery(retried.outcome)).toBeNull()
  })
})

describe("asking for the handoff at most once per attempt", () => {
  it("lets one claim through and refuses the rest", () => {
    const gate = createHandoffGate()
    // The effect running twice against one state — a development double-invoke,
    // or a re-render arriving before the dispatch does.
    expect(gate.claim()).toBe(true)
    expect(gate.claim()).toBe(false)
    expect(gate.claim()).toBe(false)
  })

  it("lets the next attempt through once the last call settled", () => {
    const gate = createHandoffGate()
    expect(gate.claim()).toBe(true)
    gate.release()
    expect(gate.claim()).toBe(true)
    // And the attempt after that is held again while it is outstanding.
    expect(gate.claim()).toBe(false)
  })

  it("is idempotent under repeated release", () => {
    const gate = createHandoffGate()
    gate.release()
    gate.release()
    expect(gate.claim()).toBe(true)
    gate.release()
    gate.release()
    expect(gate.claim()).toBe(true)
    expect(gate.claim()).toBe(false)
  })

  it("holds separately for separate surfaces", () => {
    const one = createHandoffGate()
    const other = createHandoffGate()
    expect(one.claim()).toBe(true)
    // A second setup surface has its own, so one does not block the other.
    expect(other.claim()).toBe(true)
  })
})

describe("saving again after a refused write", () => {
  it("starts a fresh attempt pointed at the write alone", () => {
    const refused = record(
      { type: "requested" },
      {
        type: "settled",
        attempt: 1,
        outcome: { outcome: "setup-not-recorded", cause: "disk full" },
      },
    )
    expect(refused.resume).toBe("hand-over")

    const saving = applySetupHandoff(refused, { type: "save-again" })
    expect(saving.resume).toBe("record")
    expect(shouldHandOver(saving, false)).toBe(true)
    // A number the outstanding attempt cannot answer to, as a retry gets.
    expect(applySetupHandoff(saving, { type: "requested" }).requested).toBe(2)
  })

  it("goes back to the whole handoff on an ordinary retry", () => {
    const saving = applySetupHandoff(noSetupHandoff, { type: "save-again" })
    expect(applySetupHandoff(saving, { type: "retry" }).resume).toBe("hand-over")
  })
})
