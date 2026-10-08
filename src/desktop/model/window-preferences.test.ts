import { describe, expect, it } from "vitest"
import { parseFlag, parseOptIn } from "./window-preferences"

describe("parseFlag", () => {
  it("is on unless turned off", () => {
    expect(parseFlag("off")).toBe("off")
    expect(parseFlag("on")).toBe("on")
    expect(parseFlag(null)).toBe("on")
    expect(parseFlag("no")).toBe("on")
  })
})

describe("parseOptIn", () => {
  it("is on only when turned on", () => {
    expect(parseOptIn("on")).toBe("on")
  })

  it("is off until then, and for anything else stored", () => {
    for (const value of [null, undefined, "off", "", "yes", 1, true])
      expect(parseOptIn(value)).toBe("off")
  })
})
