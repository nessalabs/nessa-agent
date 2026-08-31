import { describe, expect, it } from "vitest"

import {
  advance,
  closeInStrip,
  conversation,
  draftReply,
  send,
  stop,
  titleFor,
} from "./conversation"

describe("send", () => {
  it("names a new conversation after its opening line", () => {
    const next = send(conversation("c0"), "hello there", "t1")
    expect(next.title).toBe("hello there")
    expect(next.phase).toBe("thinking")
    expect(next.turns).toEqual([{ id: "t1", from: "user", text: "hello there" }])
    expect(next.pending).toBe("hello there")
    expect(next.draft).toBe("")
  })

  it("does not rename a conversation that already has a turn", () => {
    const open = send(conversation("c0"), "first", "t1")
    const idle = { ...open, phase: "idle" as const }
    const next = send(idle, "second", "t2")
    expect(next.title).toBe("first")
    expect(next.turns).toHaveLength(2)
  })

  it("ignores an empty prompt and a conversation that is already busy", () => {
    const empty = conversation("c0")
    expect(send(empty, "   ", "t1")).toBe(empty)
    const busy = send(empty, "hi", "t1")
    expect(send(busy, "again", "t2")).toBe(busy)
  })
})

describe("advance", () => {
  it("reveals the stand-in reply when thinking, then returns to idle", () => {
    const thinking = send(conversation("c0"), "hi", "t1")
    const streaming = advance(thinking, "t2")
    expect(streaming.phase).toBe("streaming")
    expect(streaming.turns.filter((turn) => turn.from === "user")).toEqual([
      { id: "t1", from: "user", text: "hi" },
    ])
    expect(streaming.turns.at(-1)).toEqual({
      id: "t2",
      from: "assistant",
      text: draftReply("hi"),
    })
    expect(advance(streaming, "t3").phase).toBe("idle")
  })

  it("leaves an idle conversation alone", () => {
    const idle = conversation("c0")
    expect(advance(idle, "t1")).toBe(idle)
  })
})

describe("stop", () => {
  it("returns a thinking conversation to idle without dropping the user turn", () => {
    const thinking = send(conversation("c0"), "hi", "t1")
    const stopped = stop(thinking)
    expect(stopped.phase).toBe("idle")
    expect(stopped.turns).toEqual(thinking.turns)
  })
})

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
})

describe("titleFor", () => {
  it("cuts a long opening line and trims the cut", () => {
    const long = "abcdefghijklmnopqrstuvwxyz"
    expect(titleFor(long)).toBe("abcdefghijklmnopqrstuvwx…")
  })
})
