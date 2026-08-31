import { describe, expect, it } from "vitest"

import {
  advanceReply,
  closeConversation,
  openConversation,
  sendDraft,
  setActiveId,
  setDraft,
  stopGenerating,
} from "./conversation-slice"
import { draftReply } from "./conversation"
import { makeStore } from "./store"

/** An agent (or a test) drives the strip by dispatching named actions. */
function agent(store: ReturnType<typeof makeStore>) {
  return {
    draft: (text: string, id?: string) =>
      store.dispatch(setDraft(id ? { draft: text, id } : { draft: text })),
    send: (id?: string) => store.dispatch(sendDraft(id ? { id } : undefined)),
    open: () => store.dispatch(openConversation()),
    close: (id: string) => store.dispatch(closeConversation(id)),
    stop: (id?: string) =>
      store.dispatch(stopGenerating(id ? { conversationId: id } : undefined)),
    tick: (id: string) => store.dispatch(advanceReply({ conversationId: id })),
    activate: (id: string) => store.dispatch(setActiveId(id)),
    strip: () => store.getState().conversation,
  }
}

describe("conversation strip store", () => {
  it("starts with one idle conversation", () => {
    const strip = makeStore().getState().conversation
    expect(strip.conversations).toHaveLength(1)
    expect(strip.activeId).toBe("c0")
    expect(strip.conversations[0]!.phase).toBe("idle")
    expect(strip.nextConversationId).toBe(1)
    expect(strip.nextTurnId).toBe(1)
  })

  it("an agent can draft and send without going through React", () => {
    const run = agent(makeStore())
    run.draft("hello there")
    run.send()
    const open = run.strip().conversations[0]!
    expect(open.phase).toBe("thinking")
    expect(open.title).toBe("hello there")
    expect(open.draft).toBe("")
    expect(open.turns[0]).toEqual({
      id: "t1",
      from: "user",
      text: "hello there",
      receipt: "sending",
    })
    expect(open.turns[1]).toEqual({
      id: "t2",
      from: "assistant",
      text: "",
    })
  })

  it("ignores send when the draft is empty or a reply is already in flight", () => {
    const run = agent(makeStore())
    run.send()
    expect(run.strip().conversations[0]!.phase).toBe("idle")
    run.draft("hi")
    run.send()
    run.draft("again")
    run.send()
    expect(run.strip().conversations[0]!.turns).toHaveLength(2)
    expect(run.strip().conversations[0]!.pending).toBe("hi")
  })

  it("walks thinking → streaming → idle when the clock dispatches", () => {
    const run = agent(makeStore())
    run.draft("hi")
    run.send()
    run.tick("c0")
    const streaming = run.strip().conversations[0]!
    expect(streaming.phase).toBe("streaming")
    expect(streaming.turns.at(-1)).toEqual({
      id: "t2",
      from: "assistant",
      text: draftReply("hi"),
    })
    run.tick("c0")
    const idle = run.strip().conversations[0]!
    expect(idle.phase).toBe("idle")
    expect(idle.pending).toBe("")
    expect(idle.turns[0]!.receipt).toBe("delivered")
  })

  it("stops a reply without dropping the user turn", () => {
    const run = agent(makeStore())
    run.draft("hi")
    run.send()
    run.stop()
    const open = run.strip().conversations[0]!
    expect(open.phase).toBe("idle")
    expect(open.pending).toBe("")
    expect(open.turns).toHaveLength(1)
    expect(open.turns[0]!.from).toBe("user")
    expect(open.turns[0]!.receipt).toBe("delivered")
  })

  it("keeps conversation ids and turn ids on separate counters", () => {
    const run = agent(makeStore())
    run.draft("hi")
    run.send()
    run.open()
    expect(run.strip().conversations.map((item) => item.id)).toEqual(["c0", "c1"])
    expect(run.strip().activeId).toBe("c1")
  })

  it("opens a tab the agent can switch to, and never empties the strip", () => {
    const run = agent(makeStore())
    run.open()
    expect(run.strip().conversations.map((item) => item.id)).toEqual(["c0", "c1"])
    expect(run.strip().activeId).toBe("c1")
    run.activate("c0")
    run.close("c0")
    expect(run.strip().activeId).toBe("c1")
    run.close("c1")
    expect(run.strip().conversations).toHaveLength(1)
    expect(run.strip().conversations[0]!.id).toBe("c2")
  })

  it("no-ops close on an unknown id", () => {
    const run = agent(makeStore())
    run.close("missing")
    expect(run.strip().conversations.map((item) => item.id)).toEqual(["c0"])
    expect(run.strip().activeId).toBe("c0")
  })

  it("keeps each conversation's turns on that conversation", () => {
    const run = agent(makeStore())
    run.draft("hello from the store")
    run.send()
    run.open()
    run.draft("second tab")
    run.send()
    const [first, second] = run.strip().conversations
    expect(first!.title).toBe("hello from the store")
    expect(second!.title).toBe("second tab")
    expect(first!.turns.map((turn) => turn.text)).toEqual(["hello from the store", ""])
    expect(second!.turns.map((turn) => turn.text)).toEqual(["second tab", ""])
    expect(first!.turns).not.toBe(second!.turns)
    run.activate("c0")
    const open = run
      .strip()
      .conversations.find((item) => item.id === run.strip().activeId)
    expect(open!.turns.map((turn) => turn.text)).toEqual(["hello from the store", ""])
  })
})
