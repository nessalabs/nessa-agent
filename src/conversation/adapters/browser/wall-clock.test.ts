import { describe, expect, it } from "vitest"
import { monotonicWallClock } from "./wall-clock"

describe("monotonicWallClock", () => {
  it("does not follow the wall clock after its one sample", () => {
    let wall = 1_000_000
    let monotonic = 50
    const now = monotonicWallClock(
      () => wall,
      () => monotonic,
    )
    expect(now()).toBe(1_000_000)
    monotonic += 8_000
    wall += 8_000 + 60_000
    expect(now()).toBe(1_008_000)
    wall -= 120_000
    monotonic += 1_000
    expect(now()).toBe(1_009_000)
  })
})
