import { describe, expect, it } from "vitest"
import { defaultIconFamily, iconFamilies, parseIconFamily } from "./icon-family"

describe("parseIconFamily", () => {
  it("keeps every known family", () => {
    for (const family of iconFamilies) expect(parseIconFamily(family.id)).toBe(family.id)
  })

  it.each([null, undefined, "", "phosphor", "toString", "constructor", 3])(
    "falls back to the default for %s",
    (value) => {
      expect(parseIconFamily(value)).toBe(defaultIconFamily)
    },
  )

  it("defaults to Nessa's own drawings", () => {
    expect(defaultIconFamily).toBe("nessa")
  })
})
