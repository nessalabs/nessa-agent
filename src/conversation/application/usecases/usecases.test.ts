import { describe, expect, it } from "vitest"

import { draftReply } from "../internal/stand-in"
import { emptyLocalStrip } from "../local-strip"
import { standInReply } from "../internal"
import {
  advanceReply,
  closeConversation,
  openConversation,
  sendDraft,
  setActive,
  setDraft,
  stopGenerating,
} from "./index"

function primed(prompt: string) {
  return sendDraft(setDraft(emptyLocalStrip(), { draft: prompt }))
}

function tick(strip: ReturnType<typeof primed>, id: string) {
  return advanceReply(strip, id, standInReply)
}

describe("sendDraft", () => {
  it("opens the assistant row from the draft", () => {
    const next = primed("hello there")
    expect(next.conversations[0]!.phase).toBe("thinking")
    expect(next.conversations[0]!.title).toBe("hello there")
    expect(next.conversations[0]!.turns).toHaveLength(2)
    expect(next.nextTurnId).toBe(3)
  })

  it("ignores an empty draft and a busy conversation", () => {
    const idle = emptyLocalStrip()
    expect(sendDraft(idle)).toBe(idle)
    const busy = primed("hi")
    expect(sendDraft(setDraft(busy, { draft: "again" }))).toEqual(
      setDraft(busy, { draft: "again" }),
    )
  })
})

describe("advanceReply", () => {
  it("does not mint an id when the conversation is idle", () => {
    const idle = emptyLocalStrip()
    expect(tick(idle, "c0")).toBe(idle)
    expect(idle.nextTurnId).toBe(1)
  })

  it("walks thinking → streaming → idle", () => {
    const thinking = primed("hi")
    const streaming = tick(thinking, "c0")
    expect(streaming.conversations[0]!.phase).toBe("streaming")
    expect(streaming.conversations[0]!.turns.at(-1)).toEqual({
      id: "t2",
      from: "assistant",
      text: draftReply("hi"),
    })
    const idle = tick(streaming, "c0")
    expect(idle.conversations[0]!.phase).toBe("idle")
    expect(idle.conversations[0]!).not.toHaveProperty("pending")
  })
})

describe("stopGenerating", () => {
  it("drops the empty assistant row and keeps the user turn", () => {
    const stopped = stopGenerating(primed("hi"))
    expect(stopped.conversations[0]!.phase).toBe("idle")
    expect(stopped.conversations[0]!.turns).toHaveLength(1)
    expect(stopped.conversations[0]!).not.toHaveProperty("pending")
  })
})

describe("openConversation / closeConversation", () => {
  it("keeps conversation ids off the turn counter", () => {
    const opened = openConversation(primed("hi"))
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
