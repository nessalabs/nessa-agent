import { describe, expect, it } from "vitest"
import { forgotten, keptConversations, remembered, removedAt } from "./retention"

describe("conversations kept", () => {
  const recency = (id: string) => Number(id.slice(1))
  const held = (count: number) =>
    Object.fromEntries(Array.from({ length: count }, (_, i) => [`s${i}`, i]))

  it("keeps everything up to the limit, as the same record", () => {
    const three = held(3)
    expect(keptConversations(three, new Set(), recency, 3)).toBe(three)
  })

  it("lets the least recent unshown go past the limit, never a shown one", () => {
    const kept = keptConversations(held(5), new Set(["s0"]), recency, 2)
    expect(Object.keys(kept).sort()).toEqual(["s0", "s3", "s4"])
  })
})

describe("removals remembered", () => {
  it("keeps the later revision of two for one session, as the newest", () => {
    const once = remembered([], { sessionId: "a", revision: 4 })
    const again = remembered(remembered(once, { sessionId: "b", revision: 1 }), {
      sessionId: "a",
      revision: 2,
    })
    expect(again).toEqual([
      { sessionId: "b", revision: 1 },
      { sessionId: "a", revision: 4 },
    ])
    expect(removedAt(again, "a")).toBe(4)
  })

  it("forgets the oldest past its limit, exactly at it", () => {
    const two = remembered(
      remembered([], { sessionId: "a", revision: 1 }, 2),
      {
        sessionId: "b",
        revision: 1,
      },
      2,
    )
    expect(two).toHaveLength(2)
    const three = remembered(two, { sessionId: "c", revision: 1 }, 2)
    expect(three.map((removal) => removal.sessionId)).toEqual(["b", "c"])
  })

  it("forgets one listed again, and holds the same list when it had none", () => {
    const one = remembered([], { sessionId: "a", revision: 1 })
    expect(forgotten(one, "a")).toEqual([])
    expect(forgotten(one, "b")).toBe(one)
  })

  it("reads a session id for what it names, whatever it looks like", () => {
    const odd = remembered([], { sessionId: "constructor", revision: 3 })
    expect(removedAt(odd, "constructor")).toBe(3)
    expect(removedAt([], "constructor")).toBeUndefined()
  })
})
