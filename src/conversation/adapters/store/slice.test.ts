import { describe, expect, it } from "vitest"

import { makeStore } from "../../../store"
import {
  closeConversation,
  openConversation,
  sendDraft,
  setActive,
  setDraft,
  stopGenerating,
} from "./slice"

/** An agent (or a test) drives the tabs by dispatching named actions. */
function agent(store: ReturnType<typeof makeStore>) {
  return {
    draft: (text: string, id?: string) =>
      store.dispatch(setDraft(id ? { draft: text, id } : { draft: text })),
    send: (id?: string) => store.dispatch(sendDraft(id ? { id } : undefined)),
    open: () => store.dispatch(openConversation()),
    close: (id: string) => store.dispatch(closeConversation(id)),
    stop: (id?: string) =>
      store.dispatch(stopGenerating(id ? { conversationId: id } : undefined)),
    activate: (id: string) => store.dispatch(setActive(id)),
    tabs: () => store.getState().conversation,
  }
}

describe("conversation tabs store", () => {
  it("starts with one idle conversation", () => {
    const tabs = makeStore().getState().conversation
    expect(tabs.conversations).toHaveLength(1)
    expect(tabs.activeId).toBe("c0")
    expect(tabs.conversations[0]!.phase).toBe("idle")
    expect(tabs.nextConversationId).toBe(1)
    expect(tabs.nextTurnId).toBe(1)
  })

  it("drafts without inventing turns; send is a no-op", () => {
    const run = agent(makeStore())
    run.draft("hello there")
    run.send()
    const open = run.tabs().conversations[0]!
    expect(open.phase).toBe("idle")
    expect(open.draft).toBe("hello there")
    expect(open.turns).toEqual([])
  })

  it("stopGenerating is a no-op", () => {
    const run = agent(makeStore())
    run.draft("hi")
    run.stop()
    expect(run.tabs().conversations[0]!.draft).toBe("hi")
    expect(run.tabs().conversations[0]!.phase).toBe("idle")
  })

  it("opens a tab the agent can switch to, and never empties the tabs", () => {
    const run = agent(makeStore())
    run.open()
    expect(run.tabs().conversations.map((item) => item.id)).toEqual(["c0", "c1"])
    expect(run.tabs().activeId).toBe("c1")
    run.activate("c0")
    run.close("c0")
    expect(run.tabs().activeId).toBe("c1")
    run.close("c1")
    expect(run.tabs().conversations).toHaveLength(1)
    expect(run.tabs().conversations[0]!.id).toBe("c2")
  })

  it("no-ops close on an unknown id", () => {
    const run = agent(makeStore())
    run.close("missing")
    expect(run.tabs().conversations.map((item) => item.id)).toEqual(["c0"])
    expect(run.tabs().activeId).toBe("c0")
  })

  it("keeps each conversation's draft on that conversation", () => {
    const run = agent(makeStore())
    run.draft("hello from the store")
    run.open()
    run.draft("second tab")
    const [first, second] = run.tabs().conversations
    expect(first!.draft).toBe("hello from the store")
    expect(second!.draft).toBe("second tab")
    run.activate("c0")
    const open = run
      .tabs()
      .conversations.find((item) => item.id === run.tabs().activeId)
    expect(open!.draft).toBe("hello from the store")
  })
})
