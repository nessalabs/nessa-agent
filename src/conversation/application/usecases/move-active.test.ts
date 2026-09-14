import { describe, expect, it } from "vitest"
import { emptyLocalTabs } from "../local-tabs"
import { openConversation, setActive, setDraft, moveActive } from "./index"

describe("relative conversation activation", () => {
  it("moves and wraps without mutating conversations or drafts", () => {
    const tabs = setDraft(openConversation(openConversation(emptyLocalTabs())), {
      draft: [{ type: "text", text: "unsent draft" }],
    })
    const ids = tabs.conversations.map((item) => item.id)
    const first = setActive(tabs, ids[0]!)
    expect(moveActive(first, 1).activeId).toBe(ids[1])
    const last = moveActive(first, -1)
    expect(last.activeId).toBe(ids[2])
    expect(moveActive(last, 1).activeId).toBe(ids[0])
    expect(last.conversations).toBe(tabs.conversations)
  })
  it("leaves single, empty, and missing-active lists unchanged", () => {
    const one = emptyLocalTabs()
    expect(moveActive(one, 1)).toBe(one)
    const empty = { ...one, conversations: [] }
    expect(moveActive(empty, -1)).toBe(empty)
    const missing = { ...openConversation(one), activeId: "missing" }
    expect(moveActive(missing, 1)).toBe(missing)
  })
})
