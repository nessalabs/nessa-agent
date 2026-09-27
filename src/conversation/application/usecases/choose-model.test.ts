import { describe, expect, it } from "vitest"
import { emptyLocalTabs } from "../local-tabs"
import { beginSend, chooseModel, openConversation } from "./index"

const opus = { agent: "claude", model: "claude-opus-5" }

describe("choosing a tab's model", () => {
  it("is kept on a tab whose conversation does not exist yet", () => {
    const tabs = openConversation(emptyLocalTabs())
    const id = tabs.activeId
    const next = chooseModel(tabs, { id, choice: opus })

    expect(next.conversations).toHaveLength(tabs.conversations.length)
    expect(next.activeId).toBe(id)
    expect(next.conversations.find((item) => item.id === id)!.modelChoice).toEqual(opus)
  })

  it("opens a new tab with it once the conversation exists, and leaves that one alone", () => {
    const tabs = beginSend(emptyLocalTabs(), {
      conversationId: "c0",
      executionId: "e",
      actionId: "a",
      mode: "queued",
      content: [{ type: "text", text: "hi" }],
    })
    const id = tabs.activeId
    const next = chooseModel(tabs, { id, choice: opus })

    expect(next.conversations).toHaveLength(tabs.conversations.length + 1)
    expect(next.conversations.find((item) => item.id === id)!.modelChoice).toBeUndefined()
    const opened = next.conversations.at(-1)!
    expect(next.activeId).toBe(opened.id)
    expect(opened.modelChoice).toEqual(opus)
  })

  it("opens a new tab when a choice lands while the conversation is being created", () => {
    const tabs = openConversation(emptyLocalTabs())
    const id = tabs.activeId
    const binding = {
      ...tabs,
      conversations: tabs.conversations.map((item) =>
        item.id === id ? { ...item, serverConversationId: "c0" } : item,
      ),
    }
    const next = chooseModel(binding, { id, choice: opus })

    expect(next.conversations.find((item) => item.id === id)!.modelChoice).toBeUndefined()
    expect(next.conversations.at(-1)!.modelChoice).toEqual(opus)
    expect(next.activeId).not.toBe(id)
  })

  it("changes nothing for a tab that is gone", () => {
    const tabs = openConversation(emptyLocalTabs())
    expect(chooseModel(tabs, { id: "gone", choice: opus })).toBe(tabs)
  })
})
