import { describe, expect, it } from "vitest"

import { conversation } from "../../model"
import { withDraft } from "./conversation"
import { closeInStrip } from "./strip"
import { titleFor } from "./title"

describe("closeInStrip", () => {
  it("replaces the last conversation rather than emptying the strip", () => {
    const only = conversation("c0")
    const next = closeInStrip([only], "c0", "c0", () => "c1")
    expect(next.items).toHaveLength(1)
    expect(next.items[0]!.id).toBe("c1")
    expect(next.activeId).toBe("c1")
  })

  it("activates a neighbour when the open conversation is closed", () => {
    const items = [conversation("c0"), conversation("c1"), conversation("c2")]
    const next = closeInStrip(items, "c1", "c1", () => "c9")
    expect(next.items.map((item) => item.id)).toEqual(["c0", "c2"])
    expect(next.activeId).toBe("c2")
  })

  it("keeps the open conversation when a background tab is closed", () => {
    const items = [conversation("c0"), conversation("c1")]
    const next = closeInStrip(items, "c1", "c0", () => "c9")
    expect(next.activeId).toBe("c0")
    expect(next.items).toHaveLength(1)
  })

  it("no-ops when the id is not in the strip", () => {
    const items = [conversation("c0")]
    const next = closeInStrip(items, "missing", "c0", () => "c9")
    expect(next.items).toBe(items)
    expect(next.activeId).toBe("c0")
  })
})

describe("withDraft", () => {
  it("writes the draft without touching phase or turns", () => {
    const current = conversation("c0")
    const next = withDraft(current, "halfway")
    expect(next.draft).toBe("halfway")
    expect(next.phase).toBe("idle")
    expect(next.turns).toEqual([])
  })
})

describe("titleFor", () => {
  it("cuts a long opening line and trims the cut", () => {
    const long = "abcdefghijklmnopqrstuvwxyz"
    expect(titleFor(long)).toBe("abcdefghijklmnopqrstuvwx…")
  })
})
