import { describe, expect, it } from "vitest"

import { emptyLocalTabs } from "../local-tabs"
import {
  beginSend,
  closeConversation,
  completeEcho,
  failSend,
  openConversation,
  setActive,
  setDraft,
  stopGenerating,
} from "./index"

describe("beginSend / completeEcho", () => {
  it("appends a user turn then an echoed assistant reply", () => {
    const pending = beginSend(emptyLocalTabs(), { text: "hey" })
    const active = pending.conversations.find((item) => item.id === pending.activeId)!
    expect(active.phase).toBe("thinking")
    expect(active.draft).toBe("")
    expect(active.turns).toEqual([
      { id: "t1", from: "user", text: "hey", receipt: "sending" },
    ])

    const done = completeEcho(pending, pending.activeId, "hey")
    const idle = done.conversations.find((item) => item.id === done.activeId)!
    expect(idle.phase).toBe("idle")
    expect(idle.turns).toEqual([
      { id: "t1", from: "user", text: "hey", receipt: "delivered" },
      { id: "t2", from: "assistant", text: "hey" },
    ])
  })

  it("no-ops an empty draft", () => {
    const tabs = emptyLocalTabs()
    expect(beginSend(tabs, { text: "  " })).toBe(tabs)
  })

  it("failSend returns to idle with a failure reply", () => {
    const pending = beginSend(emptyLocalTabs(), { text: "hey" })
    const failed = failSend(pending, pending.activeId, "not connected")
    const active = failed.conversations.find((item) => item.id === failed.activeId)!
    expect(active.phase).toBe("idle")
    expect(active.turns).toEqual([
      { id: "t1", from: "user", text: "hey", receipt: "delivered" },
      { id: "t2", from: "assistant", text: "not connected" },
    ])
  })
})

describe("stopGenerating", () => {
  it("is a no-op until stop RPCs exist", () => {
    const drafted = setDraft(emptyLocalTabs(), { draft: "hello there" })
    expect(stopGenerating(drafted)).toBe(drafted)
  })
})

describe("openConversation / closeConversation", () => {
  it("keeps conversation ids off the turn counter", () => {
    const opened = openConversation(emptyLocalTabs())
    expect(opened.conversations.map((item) => item.id)).toEqual(["c0", "c1"])
    expect(opened.activeId).toBe("c1")
  })

  it("never empties the tabs", () => {
    const only = closeConversation(emptyLocalTabs(), "c0")
    expect(only.conversations).toHaveLength(1)
    expect(only.conversations[0]!.id).toBe("c1")
    expect(only.activeId).toBe("c1")
  })

  it("no-ops an unknown id", () => {
    const tabs = emptyLocalTabs()
    const next = closeConversation(tabs, "missing")
    expect(next.conversations).toBe(tabs.conversations)
    expect(next.activeId).toBe("c0")
  })
})

describe("setActive / setDraft", () => {
  it("switches tabs and writes a draft on the open conversation", () => {
    const two = openConversation(emptyLocalTabs())
    const drafted = setDraft(two, { draft: "note", id: "c0" })
    expect(setActive(drafted, "c0").activeId).toBe("c0")
    expect(drafted.conversations[0]!.draft).toBe("note")
    expect(setActive(two, "missing")).toBe(two)
  })
})
