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
    expect(parseOptIn("off")).toBe("off")
    expect(parseOptIn(null)).toBe("off")
    expect(parseOptIn("yes")).toBe("off")
    expect(parseOptIn(1)).toBe("off")
  })
})
