import { describe, expect, it } from "vitest"

import { emptyLocalStrip } from "../local-strip"
import {
  closeConversation,
  openConversation,
  sendDraft,
  setActive,
  setDraft,
  stopGenerating,
} from "./index"

describe("sendDraft / stopGenerating", () => {
  it("are no-ops until a remote gateway owns turns", () => {
    const drafted = setDraft(emptyLocalStrip(), { draft: "hello there" })
    expect(sendDraft(drafted)).toBe(drafted)
    expect(stopGenerating(drafted)).toBe(drafted)
  })
})

describe("openConversation / closeConversation", () => {
  it("keeps conversation ids off the turn counter", () => {
    const opened = openConversation(emptyLocalStrip())
    expect(opened.conversations.map((item) => item.id)).toEqual(["c0", "c1"])
    expect(opened.activeId).toBe("c1")
  })

  it("never empties the strip", () => {
    const only = closeConversation(emptyLocalStrip(), "c0")
    expect(only.conversations).toHaveLength(1)
    expect(only.conversations[0]!.id).toBe("c1")
    expect(only.activeId).toBe("c1")
  })

  it("no-ops an unknown id", () => {
    const strip = emptyLocalStrip()
    const next = closeConversation(strip, "missing")
    expect(next.conversations).toBe(strip.conversations)
    expect(next.activeId).toBe("c0")
  })
})

describe("setActive / setDraft", () => {
  it("switches tabs and writes a draft on the open conversation", () => {
    const two = openConversation(emptyLocalStrip())
    const drafted = setDraft(two, { draft: "note", id: "c0" })
    expect(setActive(drafted, "c0").activeId).toBe("c0")
    expect(drafted.conversations[0]!.draft).toBe("note")
    expect(setActive(two, "missing")).toBe(two)
  })
})
