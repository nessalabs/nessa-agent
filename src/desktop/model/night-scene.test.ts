import { describe, expect, it } from "vitest"
import { nightSceneScale } from "./night-scene"

describe("nightSceneScale", () => {
  it("fills the height down to the sill", () => {
    expect(nightSceneScale(203, 290)).toBeCloseTo(0.7)
  })

  it("never goes above 12px or below 5px equivalent", () => {
    expect(nightSceneScale(9999, 290)).toBeCloseTo(1.2)
    expect(nightSceneScale(10, 290)).toBeCloseTo(0.5)
  })

  it("leaves an unmeasured scene at its layout size", () => {
    expect(nightSceneScale(300, 0)).toBe(1)
  })
})
