import { describe, expect, it } from "vitest"
import { parseFlag } from "./window-preferences"

describe("parseFlag", () => {
  it("is on unless turned off", () => {
    expect(parseFlag("off")).toBe("off")
    expect(parseFlag("on")).toBe("on")
    expect(parseFlag(null)).toBe("on")
    expect(parseFlag("no")).toBe("on")
  })
})
