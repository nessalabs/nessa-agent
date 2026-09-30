import { describe, expect, it } from "vitest"
import { points } from "./format"

describe("a change in points", () => {
  it("is always signed, with a true minus", () => {
    expect(points(1.84)).toBe("+1.8")
    expect(points(-0.44)).toBe("−0.4")
    expect(points(0.04)).toBe("±0.0")
  })
})
