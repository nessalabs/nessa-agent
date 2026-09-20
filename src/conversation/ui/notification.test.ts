import { ConversationErrorCode } from "@nessa/client"
import { describe, expect, it } from "vitest"
import { conversation } from "../model"
import { conversationNotice } from "./notification"

describe("conversation notification", () => {
  it("keeps uncertain admission retry tied to its original execution", () => {
    const value = conversation("tab")
    value.error = "Not connected"
    value.readError = "Not connected"
    value.turns = [
      {
        id: "turn",
        from: "user",
        content: [],
        receipt: "unknown",
        executionId: "execution",
        actionId: "original",
      },
    ]
    expect(conversationNotice(value)?.retry).toEqual({
      kind: "submission",
      executionId: "execution",
    })
    expect(conversationNotice(value)?.title).toBe("Delivery unknown")
    expect(value.turns[0]).toMatchObject({ actionId: "original", receipt: "unknown" })
  })
  it("offers refresh rather than replaying an uncertain control action", () => {
    const value = conversation("tab")
    value.error = "Close acknowledgement lost"
    expect(conversationNotice(value)?.retry).toEqual({ kind: "refresh" })
  })
  it("retries the preserved draft only for a known failed admission", () => {
    const value = conversation("tab")
    value.error = "offline"
    value.draft = [{ type: "text", text: "draft" }]
    value.turns = [
      { id: "turn", from: "user", content: [], receipt: "failed", error: "offline" },
    ]
    expect(conversationNotice(value)).toMatchObject({
      title: "Message not sent",
      retry: { kind: "draft" },
    })
  })
  it("says the agent was still starting when the gateway rejected the send", () => {
    const value = conversation("tab")
    value.error = "The agent was still starting and ran out of time, so nothing was sent."
    value.errorCode = ConversationErrorCode.AgentStartupDeadline
    value.draft = [{ type: "text", text: "draft" }]
    value.turns = [
      {
        id: "turn",
        from: "user",
        content: [],
        receipt: "failed",
        error: value.error,
      },
    ]
    const notice = conversationNotice(value)
    expect(notice).toMatchObject({
      title: "Agent was still starting",
      retry: { kind: "draft" },
    })
    expect(notice?.description).toContain(value.error)
    expect(notice?.description).toContain("Retry sends the current draft.")
  })
  it("explains a configuration mismatch without offering an ineffective retry", () => {
    const value = conversation("tab")
    value.readError = "conversation_configuration_changed"
    expect(conversationNotice(value)).toMatchObject({
      title: "Conversation setup changed",
      retry: null,
    })
  })
  it("does not show a notification for a healthy conversation", () => {
    expect(conversationNotice(conversation("tab"))).toBeNull()
  })
})
