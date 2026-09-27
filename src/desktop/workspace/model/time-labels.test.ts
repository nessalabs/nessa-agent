import { describe, expect, it } from "vitest"
import { elapsed, sessionTime, startedLabel } from "./time-labels"

// Local times, so the day boundaries are the reader's own whatever the zone.
const at = (day: number, hour: number, minute = 0) =>
  new Date(2026, 8, day, hour, minute).getTime()
// Sunday 27 September 2026, 21:00.
const now = at(27, 21)

describe("a session's time", () => {
  it("says now, minutes and hours while it is recent", () => {
    expect(sessionTime(now - 30_000, now)).toBe("now")
    expect(sessionTime(at(27, 20, 56), now)).toBe("4m")
    expect(sessionTime(at(27, 18), now)).toBe("3h")
    expect(sessionTime(at(27, 0, 30), now)).toBe("20h")
  })

  it("says hours for a late night still within twelve of now", () => {
    expect(sessionTime(at(27, 1), at(27, 9))).toBe("8h")
    expect(sessionTime(at(26, 23), at(27, 8))).toBe("9h")
  })

  it("says Yesterday, then the weekday, then the date", () => {
    expect(sessionTime(at(26, 8), now)).toBe("Yesterday")
    expect(sessionTime(at(21, 10), now)).toBe("Mon")
    expect(sessionTime(at(19, 10), now)).toBe("Sep 19")
  })

  it("treats a time in the future as now", () => {
    expect(sessionTime(now + 60_000, now)).toBe("now")
  })
})

describe("when a conversation started", () => {
  it("reads as a phrase, capitalised when it leads", () => {
    expect(startedLabel(now, now)).toBe("started just now")
    expect(startedLabel(at(27, 20, 38), now, true)).toBe("Started 22m ago")
    expect(startedLabel(at(26, 8), now)).toBe("started yesterday")
    expect(startedLabel(at(21, 10), now)).toBe("started Mon")
  })
})

describe("how long an agent has been at it", () => {
  it("counts seconds, then minutes and seconds", () => {
    expect(elapsed(now - 39_000, now)).toBe("39s")
    expect(elapsed(now - 95_000, now)).toBe("1m 35s")
    expect(elapsed(now + 5_000, now)).toBe("0s")
  })
})
