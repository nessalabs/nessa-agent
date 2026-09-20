import { ConversationErrorCode } from "@nessa/client"
import type { Conversation } from "../model"

export type ConversationNotice = {
  title: string
  description: string
  retry:
    | { kind: "submission"; executionId: string }
    | { kind: "refresh" }
    | { kind: "draft" }
    | null
}

/** One notification, while individual turns retain their own delivery receipts. */
export function conversationNotice(
  conversation: Conversation,
): ConversationNotice | null {
  const unknown = conversation.turns.find(
    (turn) => turn.from === "user" && turn.receipt === "unknown" && turn.executionId,
  )
  if (unknown?.from === "user" && unknown.executionId)
    return {
      title: "Delivery unknown",
      description:
        "The gateway may have received your message. Retry checks the same submission; reconnecting does not resend it.",
      retry: { kind: "submission", executionId: unknown.executionId },
    }
  const unsent = conversation.turns.some(
    (turn) =>
      turn.from === "user" &&
      turn.receipt === "failed" &&
      turn.error === conversation.error,
  )
  if (unsent)
    return {
      title:
        conversation.errorCode === ConversationErrorCode.AgentStartupDeadline
          ? "Agent was still starting"
          : "Message not sent",
      description: `${conversation.error} Your draft is still here. Retry sends the current draft.`,
      retry: conversation.draft.length ? { kind: "draft" } : null,
    }
  const error =
    conversation.remote?.permissionViewError ??
    conversation.error ??
    conversation.readError
  if (!error) return null
  if (error === ConversationErrorCode.ConversationConfigurationChanged)
    return {
      title: "Conversation setup changed",
      description:
        "This chat uses a different agent configuration. Start a new conversation with the current setup.",
      retry: null,
    }
  return {
    title: "Conversation needs attention",
    description: error,
    retry: { kind: "refresh" },
  }
}
