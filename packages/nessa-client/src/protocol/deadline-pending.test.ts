import { describe, expect, it } from "vitest"
import { deadlinePendingLine } from "./deadline-pending.js"

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
