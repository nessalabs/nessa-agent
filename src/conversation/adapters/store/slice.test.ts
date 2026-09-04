import { describe, expect, it, vi, beforeEach, afterEach } from "vitest"

import { makeStore } from "../../../store"
import { setSessionClient } from "../../../session/adapters/client/handle"
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
    send: async (text: string, id?: string) => {
      await store.dispatch(sendDraft(id ? { text, id } : { text }))
    },
    open: () => store.dispatch(openConversation()),
    close: (id: string) => store.dispatch(closeConversation(id)),
    stop: (id?: string) =>
      store.dispatch(stopGenerating(id ? { conversationId: id } : undefined)),
    activate: (id: string) => store.dispatch(setActive(id)),
    tabs: () => store.getState().conversation,
  }
}

describe("conversation tabs store", () => {
  beforeEach(() => {
    setSessionClient(null)
  })
  afterEach(() => {
    setSessionClient(null)
  })

  it("starts with one idle conversation", () => {
    const tabs = makeStore().getState().conversation
    expect(tabs.conversations).toHaveLength(1)
    expect(tabs.activeId).toBe("c0")
    expect(tabs.conversations[0]!.phase).toBe("idle")
    expect(tabs.nextConversationId).toBe(1)
    expect(tabs.nextTurnId).toBe(1)
  })

  it("echoes a send through the session client into turns", async () => {
    setSessionClient({
      conversation: {
        echo: vi.fn().mockResolvedValue({ text: "hey" }),
      },
    } as never)

    const run = agent(makeStore())
    run.draft("hey")
    await run.send("hey")
    const open = run.tabs().conversations[0]!
    expect(open.phase).toBe("idle")
    expect(open.draft).toBe("")
    expect(open.turns).toEqual([
      { id: "t1", from: "user", text: "hey", receipt: "delivered" },
      { id: "t2", from: "assistant", text: "hey" },
    ])
  })

  it("keeps the user turn when echo fails", async () => {
    setSessionClient({
      conversation: {
        echo: vi.fn().mockRejectedValue(new Error("offline")),
      },
    } as never)

    const run = agent(makeStore())
    run.draft("hey")
    await run.send("hey")
    const open = run.tabs().conversations[0]!
    expect(open.phase).toBe("idle")
    expect(open.turns).toEqual([
      { id: "t1", from: "user", text: "hey", receipt: "delivered" },
    ])
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
