import { describe, expect, it } from "vitest"
import { tagline, taglines } from "./tagline"

describe("taglines", () => {
  it("picks the same line for the same seed", () => {
    expect(tagline("mara")).toBe(tagline("mara"))
    expect(tagline("mara")).not.toBe(tagline("idris"))
    expect(tagline("\uD800")).toBe(tagline("\uD800"))
  })

  it("reaches every line", () => {
    const reached = new Set<string>()
    for (let seed = 0; seed < 400 && reached.size < taglines.length; seed += 1)
      reached.add(tagline(String(seed)))
    expect([...reached].sort()).toEqual([...taglines].sort())
  })
})
