import { describe, expect, it } from "vitest"
import { tabAfter, tabAt } from "./tab-navigation"

/** What the strip shows while an update is downloading beside two chats. */
const strip = ["a", "b", "update"]

describe("tabAfter", () => {
  it("steps through every tab the strip shows, including the update", () => {
    expect(tabAfter(strip, "a", 1)).toBe("b")
    // Was "a": the update was in the strip and not in the walk.
    expect(tabAfter(strip, "b", 1)).toBe("update")
    expect(tabAfter(strip, "update", -1)).toBe("b")
    expect(tabAfter(strip, "b", -1)).toBe("a")
  })

  it("wraps at both ends", () => {
    expect(tabAfter(strip, "update", 1)).toBe("a")
    expect(tabAfter(strip, "a", -1)).toBe("update")
  })

  it("still moves when the selection has gone", () => {
    // A conversation closing as the key is pressed leaves a selection that is
    // no longer in the strip; the shortcut should move, not stall.
    expect(tabAfter(strip, "closed-just-now", 1)).toBe("a")
    expect(tabAfter(strip, "closed-just-now", -1)).toBe("update")
  })

  it("has nowhere to go in an empty strip", () => {
    expect(tabAfter([], "a", 1)).toBeUndefined()
  })

  it("stays put in a strip of one", () => {
    expect(tabAfter(["a"], "a", 1)).toBe("a")
    expect(tabAfter(["a"], "a", -1)).toBe("a")
  })
})

describe("tabAt", () => {
  it("counts the positions the strip actually has", () => {
    expect(tabAt(strip, 0)).toBe("a")
    // Was undefined: the third position is the update tab.
    expect(tabAt(strip, 2)).toBe("update")
    expect(tabAt(strip, 3)).toBeUndefined()
  })
})
