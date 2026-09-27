import { describe, expect, it } from "vitest"
import { emptyLocalTabs } from "../local-tabs"
import { beginSend, chooseApproval, openConversation } from "./index"

describe("choosing a tab's approval mode", () => {
  it("is kept on a tab whose conversation does not exist yet", () => {
    const tabs = openConversation(emptyLocalTabs())
    const id = tabs.activeId
    const next = chooseApproval(tabs, { id, mode: "auto" })

    expect(next.conversations.find((item) => item.id === id)!.approvalChoice).toBe("auto")
  })

  it("leaves a conversation that exists alone: changing it is the gateway's", () => {
    const tabs = beginSend(emptyLocalTabs(), {
      conversationId: "c0",
      executionId: "e",
      actionId: "a",
      mode: "queued",
      content: [{ type: "text", text: "hi" }],
    })
    expect(chooseApproval(tabs, { id: tabs.activeId, mode: "full" })).toBe(tabs)
  })

  it("changes nothing for a tab that is gone", () => {
    const tabs = openConversation(emptyLocalTabs())
    expect(chooseApproval(tabs, { id: "gone", mode: "auto" })).toBe(tabs)
  })
})
