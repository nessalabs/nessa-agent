import { describe, expect, it } from "vitest"
import { ago, count, exact, plural, running } from "./format"

describe("a count", () => {
  it("is written whole below a thousand, and compact from there", () => {
    expect(count(0)).toBe("0")
    expect(count(840)).toBe("840")
    expect(count(999)).toBe("999")
    expect(count(1000)).toBe("1K")
    expect(count(1284)).toBe("1.3K")
    expect(count(10482)).toBe("10.5K")
    expect(count(100_000)).toBe("100K")
    expect(count(1_240_000)).toBe("1.2M")
  })

  it("is exact where it has to be", () => {
    expect(exact(10482)).toBe("10,482")
  })

  it("agrees with its noun", () => {
    expect(plural(1, "file")).toBe("1 file")
    expect(plural(1240, "file")).toBe("1.2K files")
    expect(plural(3, "more folder", "more folders")).toBe("3 more folders")
  })
})

describe("a time", () => {
  const now = 10 * 3_600_000
  it("says how long ago, briefly", () => {
    expect(ago(now - 20_000, now)).toBe("now")
    expect(ago(now - 4 * 60_000, now)).toBe("4m")
    expect(ago(now - 130 * 60_000, now)).toBe("2h 10m")
  })

  it("says how long something has been going", () => {
    expect(running(now - 38_000, now)).toBe("38s")
    expect(running(now - 252_000, now)).toBe("4m 12s")
  })
})
