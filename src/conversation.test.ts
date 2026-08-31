import { describe, expect, it } from "vitest"

import {
  advance,
  closeInStrip,
  conversation,
  draftReply,
  send,
  stop,
  titleFor,
  withDraft,
} from "./conversation"

describe("send", () => {
  it("names a new conversation after its opening line and opens an assistant row", () => {
    const next = send(conversation("c0"), "hello there", "t1", "t2")
    expect(next.title).toBe("hello there")
    expect(next.phase).toBe("thinking")
    expect(next.turns).toEqual([
      { id: "t1", from: "user", text: "hello there", receipt: "sending" },
      { id: "t2", from: "assistant", text: "" },
    ])
    expect(next.pending).toBe("hello there")
    expect(next.draft).toBe("")
  })

  it("does not rename a conversation that already has a turn", () => {
    const open = send(conversation("c0"), "first", "t1", "t2")
    const settled = { ...open, phase: "idle" as const, pending: "" }
    const next = send(settled, "second", "t3", "t4")
    expect(next.title).toBe("first")
    expect(next.turns).toHaveLength(4)
  })

  it("ignores an empty prompt and a conversation that is already busy", () => {
    const empty = conversation("c0")
    expect(send(empty, "   ", "t1", "t2")).toBe(empty)
    const busy = send(empty, "hi", "t1", "t2")
    expect(send(busy, "again", "t3", "t4")).toBe(busy)
  })

  it("can send again after a stop", () => {
    const stopped = stop(send(conversation("c0"), "hi", "t1", "t2"))
    const next = send(stopped, "again", "t3", "t4")
    expect(next.phase).toBe("thinking")
    expect(next.turns.map((turn) => turn.text)).toEqual(["hi", "again", ""])
  })
})

describe("advance", () => {
  it("fills the open assistant row, then returns to idle", () => {
    const thinking = send(conversation("c0"), "hi", "t1", "t2")
    const streaming = advance(thinking, "t9")
    expect(streaming.phase).toBe("streaming")
    expect(streaming.turns.filter((turn) => turn.from === "user")).toEqual([
      { id: "t1", from: "user", text: "hi", receipt: "sending" },
    ])
    expect(streaming.turns.at(-1)).toEqual({
      id: "t2",
      from: "assistant",
      text: draftReply("hi"),
    })
    const idle = advance(streaming, "t10")
    expect(idle.phase).toBe("idle")
    expect(idle.pending).toBe("")
    expect(idle.turns[0]).toEqual({
      id: "t1",
      from: "user",
      text: "hi",
      receipt: "delivered",
    })
  })

  it("leaves an idle conversation alone", () => {
    const idle = conversation("c0")
    expect(advance(idle, "t1")).toBe(idle)
  })
})

describe("stop", () => {
  it("returns a thinking conversation to idle and drops the empty assistant row", () => {
    const thinking = send(conversation("c0"), "hi", "t1", "t2")
    const stopped = stop(thinking)
    expect(stopped.phase).toBe("idle")
    expect(stopped.pending).toBe("")
    expect(stopped.turns).toEqual([
      { id: "t1", from: "user", text: "hi", receipt: "delivered" },
    ])
  })

  it("leaves an idle conversation alone", () => {
    const idle = conversation("c0")
    expect(stop(idle)).toBe(idle)
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
