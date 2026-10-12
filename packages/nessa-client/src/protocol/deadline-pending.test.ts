import { describe, expect, it } from "vitest"
import { clientDueAtMs, deadlinePendingLine } from "./deadline-pending.js"

const dueAtMs = 1_700_000_030_000

describe("deadlinePendingLine", () => {
  it("says what will happen, from the outcome and the time left", () => {
    expect(
      deadlinePendingLine(
        { dueAtMs, effect: "deny", stopsTurn: false },
        dueAtMs - 30_000,
      ),
    ).toBe("If nobody answers within 30 seconds, this is denied automatically.")
    expect(
      deadlinePendingLine(
        { dueAtMs, effect: "allow", stopsTurn: false },
        dueAtMs - 8_000,
      ),
    ).toBe("If nobody answers within 8 seconds, this is allowed automatically.")
    expect(
      deadlinePendingLine({ dueAtMs, effect: "deny", stopsTurn: false }, dueAtMs - 1_000),
    ).toBe("If nobody answers within 1 second, this is denied automatically.")
    expect(
      deadlinePendingLine({ dueAtMs, effect: "deny", stopsTurn: true }, dueAtMs - 30_000),
    ).toBe(
      "If nobody answers within 30 seconds, this is denied automatically and the agent stops.",
    )
  })

  it("drops the count once the deadline has passed", () => {
    expect(
      deadlinePendingLine({ dueAtMs, effect: "deny", stopsTurn: false }, dueAtMs),
    ).toBe("If nobody answers, this is denied automatically.")
    expect(
      deadlinePendingLine({ dueAtMs, effect: "deny", stopsTurn: true }, dueAtMs + 5_000),
    ).toBe("If nobody answers, this is denied automatically and the agent stops.")
  })
})

describe("clientDueAtMs", () => {
  // Due 30 s after the gateway sent the view, on the gateway's clock.
  const serverNowMs = dueAtMs - 30_000

  it("counts down from the gateway's time left whatever this client's clock says", () => {
    for (const skewMs of [0, 60_000, -60_000, 3_600_000]) {
      const receivedAtMs = serverNowMs + skewMs
      const due = clientDueAtMs(dueAtMs, serverNowMs, receivedAtMs)
      expect(due - receivedAtMs, `skew ${skewMs}`).toBe(30_000)
      expect(
        deadlinePendingLine(
          { dueAtMs: due, effect: "deny", stopsTurn: false },
          receivedAtMs,
        ),
      ).toBe("If nobody answers within 30 seconds, this is denied automatically.")
      expect(
        deadlinePendingLine(
          { dueAtMs: due, effect: "deny", stopsTurn: false },
          receivedAtMs + 30_000,
        ),
      ).toBe("If nobody answers, this is denied automatically.")
    }
  })

  it("keeps a deadline the gateway already passed as passed", () => {
    const late = serverNowMs + 31_000
    const receivedAtMs = late - 60_000
    const due = clientDueAtMs(dueAtMs, late, receivedAtMs)
    expect(due).toBeLessThan(receivedAtMs)
  })
})
