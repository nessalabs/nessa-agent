import { describe, expect, it } from "vitest"
import { conversation, type AgentFeatures } from "../model"
import { conversationNotice } from "./notification"

const agentFeatures: AgentFeatures = {
  permissionDenial: "unknown",
  nativeHookSuppression: "unknown",
  compactionReporting: "unsupported_not_implemented",
  modelSwitchReporting: "unsupported_not_implemented",
  permissionDeferral: "unsupported_not_implemented",
  elicitationForwarding: "unknown",
  preToolPolicy: "unsupported_not_implemented",
  policyEndTurn: "unsupported_not_implemented",
  policyCloseSession: "unsupported_not_implemented",
  incomingElicitation: "unsupported_not_implemented",
}

it("shows a late bounded startup failure from the replacement view", () => {
  const value = conversation("tab")
  value.remote = {
    questions: [],
    running: false,
    permissions: [],
    tools: [],
    pending: [],
    capabilities: {
      queue: true,
      steer: true,
      resume: false,
      permissions: false,
      imageInput: false,
      agentFeatures,
    },
    lifecycle: {
      phase: "failed",
      failure: { code: "provider", message: "The configured agent did not start." },
    },
    queueComplete: true,
    truncated: false,
  }
  expect(conversationNotice(value)).toEqual({
    title: "Agent could not start",
    description: "The configured agent did not start.",
    retry: { kind: "refresh" },
  })
})

it("shows late mandatory lifecycle evidence failure after attachment succeeds", () => {
  const value = conversation("tab")
  value.remote = {
    questions: [],
    running: false,
    permissions: [],
    tools: [],
    pending: [],
    capabilities: {
      queue: true,
      steer: true,
      resume: false,
      permissions: false,
      imageInput: false,
      agentFeatures,
    },
    lifecycle: {
      phase: "attached",
      evidenceFailure: {
        code: "audit",
        message: "Attachment audit was not acknowledged.",
      },
    },
    queueComplete: true,
    truncated: false,
  }
  expect(conversationNotice(value)).toEqual({
    title: "Agent lifecycle record failed",
    description: "Attachment audit was not acknowledged.",
    retry: { kind: "refresh" },
  })
})

describe("conversation notification", () => {
  it("keeps uncertain admission retry tied to its original execution", () => {
    const value = conversation("tab")
    value.error = "Not connected"
    value.readError = "unavailable"
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
    value.failure = "agent-startup-deadline"
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
  it("says the same for a control that found the agent still starting", () => {
    const value = conversation("tab")
    // A control fails before any turn is sent, so there is no failed turn and
    // no draft to restore; the notice must still name the cause.
    value.error = "The agent was still starting and ran out of time."
    value.failure = "agent-startup-deadline"
    expect(conversationNotice(value)).toEqual({
      title: "Agent was still starting",
      description: value.error,
      retry: { kind: "refresh" },
    })
  })
  it("says a control failed in its own words without claiming the agent was starting", () => {
    const value = conversation("tab")
    // A close whose cleanup did not finish is not a startup deadline and not a
    // refusal; it gets the ordinary heading and the sentence the store chose.
    value.error =
      "The conversation stopped, but the gateway could not release the images it had stored for it."
    value.failure = "attachment-cleanup-unavailable"
    expect(conversationNotice(value)).toEqual({
      title: "Conversation needs attention",
      description: value.error,
      retry: { kind: "refresh" },
    })
  })
  it("explains a configuration mismatch without offering an ineffective retry", () => {
    const value = conversation("tab")
    // The panel's own word, decided in the gateway adapter. Nothing here
    // compares a message against `conversation_configuration_changed`, so a
    // gateway that started sending a human sentence beside that code would not
    // quietly take this notice away.
    value.readError = "configuration-changed"
    expect(conversationNotice(value)).toEqual({
      title: "Conversation setup changed",
      description:
        "This chat uses a different agent configuration. Start a new conversation with the current setup.",
      retry: null,
    })
  })
  it("promises no recovery for a gateway that may never serve this conversation again", () => {
    const value = conversation("tab")
    // `temporarily_unavailable` and `agent_startup_deadline` both land here, and
    // the gateway sends both for a conversation whose slot it has deliberately
    // retained. Nothing may say it will catch up.
    value.readError = "unavailable"
    const notice = conversationNotice(value)
    expect(notice?.description).not.toMatch(/catch(es)? up|in a moment|shortly|soon/)
    expect(notice?.description).toContain("It keeps trying.")
  })
  it("says only that the view is stale when the read failed for a reason it cannot name", () => {
    const value = conversation("tab")
    // What a code this build has never heard of becomes. The sentence claims
    // nothing about the cause, and names no code.
    value.readError = "unavailable"
    const notice = conversationNotice(value)
    expect(notice).toMatchObject({
      title: "Conversation not refreshed",
      retry: { kind: "refresh" },
    })
    expect(notice?.description).toBe(
      "Nessa could not read this conversation from the gateway, so what is shown may be out of date. It keeps trying.",
    )
  })
  it("lets a command somebody asked for outrank a read that failed behind it", () => {
    const value = conversation("tab")
    // Both states at once: a control failed, and the refresh it triggered failed
    // too. The command's sentence is the one somebody is waiting for, and an
    // ordinary stale view adds nothing to it.
    value.error = "The gateway would not take this action, so nothing was done."
    value.failure = "invalid-request"
    value.readError = "unavailable"
    expect(conversationNotice(value)).toEqual({
      title: "Conversation needs attention",
      description: value.error,
      retry: { kind: "refresh" },
    })
  })
  it("tells somebody whose saved conversation cannot be read, whatever else failed", () => {
    // The word had a notice written for it and the notice was unreachable in the
    // only case that produces it. A gateway that cannot read a conversation's
    // saved state caches that failure and answers *every* command from it, so a
    // command error always sits beside this read error — and the guard above
    // was `readError === "configuration-changed"`, one word rather than the
    // vocabulary. What arrived instead was "Conversation needs attention" and a
    // Refresh that could not work: the regression #114 shipped to fix, back for
    // the new word.
    const alone = conversation("tab")
    alone.readError = "state-unreadable"
    const beside = conversation("tab")
    beside.readError = "state-unreadable"
    beside.error = "Nessa cannot read this conversation's saved state."
    beside.failure = "conversation-state-unreadable"
    const withAnUnsentDraft = conversation("tab")
    withAnUnsentDraft.readError = "state-unreadable"
    withAnUnsentDraft.error = "offline"
    withAnUnsentDraft.draft = [{ type: "text", text: "draft" }]
    withAnUnsentDraft.turns = [
      { id: "turn", from: "user", content: [], receipt: "failed", error: "offline" },
    ]
    for (const value of [alone, beside, withAnUnsentDraft])
      expect(conversationNotice(value)).toEqual({
        title: "Conversation cannot be read",
        description:
          "Nessa cannot read this conversation's saved state, so it cannot be opened again. Start a new conversation to carry on.",
        retry: null,
      })
  })
  it("says a deleted conversation is gone, above any command, with nothing to press", () => {
    const beside = conversation("tab")
    beside.readError = "deleted"
    beside.error = "This conversation was deleted, so the message was not sent."
    beside.failure = "conversation-deleted"
    expect(conversationNotice(beside)).toEqual({
      title: "Conversation deleted",
      description:
        "This conversation was deleted, so it cannot be continued. Close this tab; anything in the draft is still here to copy.",
      retry: null,
    })
  })
  it("offers no action for any read failure the gateway will not answer differently", () => {
    // Stated over the vocabulary rather than one word at a time: whichever read
    // failures are permanent, none of them may end in something to press. A
    // notice that keeps offering a refresh for a conversation that cannot be
    // read is the whole defect, and it is not specific to either word.
    for (const reason of ["configuration-changed", "state-unreadable"] as const) {
      const value = conversation("tab")
      value.readError = reason
      value.error = "something else also failed"
      const notice = conversationNotice(value)
      expect(notice?.retry, reason).toBeNull()
      expect(notice?.description, reason).toMatch(/[Ss]tart a new conversation/)
    }
  })
  it("lets a conversation the gateway will not serve again outrank every other notice", () => {
    // The exception, and the reason it is one: every other notice on this tab
    // ends in an action — refresh, resend, retry this submission — that a
    // conversation the gateway has stopped serving cannot complete. A lost close
    // acknowledgement asking for a refresh is the concrete case.
    const lostAcknowledgement = conversation("tab")
    lostAcknowledgement.error = "Close acknowledgement lost"
    lostAcknowledgement.readError = "configuration-changed"
    const unsentDraft = conversation("tab")
    unsentDraft.error = "offline"
    unsentDraft.draft = [{ type: "text", text: "draft" }]
    unsentDraft.turns = [
      { id: "turn", from: "user", content: [], receipt: "failed", error: "offline" },
    ]
    unsentDraft.readError = "configuration-changed"
    const unknownDelivery = conversation("tab")
    unknownDelivery.turns = [
      {
        id: "turn",
        from: "user",
        content: [],
        receipt: "unknown",
        executionId: "execution",
      },
    ]
    unknownDelivery.readError = "configuration-changed"
    for (const value of [lostAcknowledgement, unsentDraft, unknownDelivery])
      expect(conversationNotice(value)).toEqual({
        title: "Conversation setup changed",
        description:
          "This chat uses a different agent configuration. Start a new conversation with the current setup.",
        retry: null,
      })
    // What became of each message is not lost with the notice slot: the turn
    // keeps its own receipt, which the transcript renders beside it.
    expect(unknownDelivery.turns[0]).toMatchObject({ receipt: "unknown" })
    expect(unsentDraft.turns[0]).toMatchObject({ receipt: "failed" })
  })
  it("does not show a notification for a healthy conversation", () => {
    expect(conversationNotice(conversation("tab"))).toBeNull()
  })
})
