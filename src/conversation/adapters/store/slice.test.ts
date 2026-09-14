import { textContent, type MessageContent } from "../../model"
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest"

import { makeStore as buildStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
let dependencies = createDependencies()
const makeStore = () => buildStore(dependencies)
const setSessionClient: typeof dependencies.session.set = (client) =>
  dependencies.session.set(client)
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
      store.dispatch(
        setDraft(id ? { draft: textContent(text), id } : { draft: textContent(text) }),
      ),
    send: async (text: string, id?: string) => {
      await store.dispatch(sendDraft({ content: textContent(text), id }))
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
    dependencies = createDependencies()
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
    expect(open.draft).toEqual(textContent(""))
    expect(open.turns).toEqual([
      {
        id: "t1",
        from: "user",
        content: textContent("hey"),
        receipt: "delivered",
      },
      { id: "t2", from: "assistant", text: "hey" },
    ])
  })

  it("keeps the user turn and shows a failure reply when echo fails", async () => {
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
      {
        id: "t1",
        from: "user",
        content: textContent("hey"),
        receipt: "delivered",
      },
      { id: "t2", from: "assistant", text: "offline" },
    ])
  })

  it("stopGenerating is a no-op", () => {
    const run = agent(makeStore())
    run.draft("hi")
    run.stop()
    expect(run.tabs().conversations[0]!.draft).toEqual(textContent("hi"))
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
    expect(first!.draft).toEqual(textContent("hello from the store"))
    expect(second!.draft).toEqual(textContent("second tab"))
    run.activate("c0")
    const open = run.tabs().conversations.find((item) => item.id === run.tabs().activeId)
    expect(open!.draft).toEqual(textContent("hello from the store"))
  })
})

it("preserves pasted-only content through tab switches, send, and a later draft", async () => {
  let finish!: (value: { text: string }) => void
  const echo = vi.fn(
    () =>
      new Promise<{ text: string }>((resolve) => {
        finish = resolve
      }),
  )
  const deps = createDependencies()
  deps.session.set({ conversation: { echo } } as never)
  const store = buildStore(deps)
  const content: MessageContent = [
    { type: "pasted-text", id: "paste", text: "  code\n\n<tag>\t\n" },
  ]
  store.dispatch(setDraft({ draft: content, id: "c0" }))
  store.dispatch(openConversation())
  store.dispatch(setActive("c0"))
  expect(store.getState().conversation.conversations[0]!.draft).toEqual(content)
  const sent = store.dispatch(sendDraft({ content, id: "c0" }))
  expect(echo).toHaveBeenCalledWith(content[0]!.text)
  expect(store.getState().conversation.conversations[0]!.draft).toEqual([])
  store.dispatch(setDraft({ draft: textContent("next draft"), id: "c0" }))
  finish({ text: "received" })
  await sent
  const conversation = store.getState().conversation.conversations[0]!
  expect(conversation.draft).toEqual(textContent("next draft"))
  expect(conversation.title).toBe("code\n\n<tag>")
  expect(conversation.turns[0]).toEqual({
    id: "t1",
    from: "user",
    content,
    receipt: "delivered",
  })
  deps.session.set(null)
})
