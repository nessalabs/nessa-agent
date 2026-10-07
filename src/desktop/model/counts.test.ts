import { describe, expect, it } from "vitest"
import { counted, countLabel, exactCount, plural } from "./counts"

describe("counts", () => {
  it("says a small count in full and a large one briefly", () => {
    expect(countLabel(0)).toBe("0")
    expect(countLabel(840)).toBe("840")
    expect(countLabel(1300)).toBe("1.3K")
    expect(exactCount(10482)).toBe("10,482")
  })

  it("pluralises one and many", () => {
    expect(plural(1, "agent")).toBe("1 agent")
    expect(plural(2, "agent")).toBe("2 agents")
    expect(plural(1, "child", "children")).toBe("1 child")
    expect(plural(3, "child", "children")).toBe("3 children")
  })

  it("keeps a word that does not change", () => {
    expect(counted(1, "working")).toBe("1 working")
    expect(counted(2, "working")).toBe("2 working")
  })
})
