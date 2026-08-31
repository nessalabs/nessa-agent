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
import { makeStore } from "./store"

/** An agent (or a test) drives the strip by dispatching named actions. */
function agent(store: ReturnType<typeof makeStore>) {
  return {
    draft: (text: string, id?: string) =>
      store.dispatch(setDraft(id ? { draft: text, id } : { draft: text })),
    send: (id?: string) => store.dispatch(sendDraft(id ? { id } : undefined)),
    open: () => store.dispatch(openConversation()),
    close: (id: string) => store.dispatch(closeConversation(id)),
    stop: (id?: string) => store.dispatch(stopGenerating(id ? { id } : undefined)),
    tick: (id: string) => store.dispatch(advanceReply({ id })),
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
    expect(run.strip().conversations[0]!.turns).toHaveLength(1)
  })

  it("walks thinking → streaming → idle when the clock dispatches", () => {
    const run = agent(makeStore())
    run.draft("hi")
    run.send()
    run.tick("c0")
    expect(run.strip().conversations[0]!.phase).toBe("streaming")
    run.tick("c0")
    expect(run.strip().conversations[0]!.phase).toBe("idle")
  })

  it("stops a reply without dropping the user turn", () => {
    const run = agent(makeStore())
    run.draft("hi")
    run.send()
    run.stop()
    const open = run.strip().conversations[0]!
    expect(open.phase).toBe("idle")
    expect(open.turns).toHaveLength(1)
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
})
