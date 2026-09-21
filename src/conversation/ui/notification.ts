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

/**
 * One notification, while individual turns retain their own delivery receipts.
 *
 * Why a command failed is asked of `conversation.failure`, the panel's own word
 * for it, which `adapters/gateway/effects.ts` translated from the gateway's.
 * Reads are the exception still outstanding: a failed read leaves only the
 * client's sentence in `readError`, and for a changed configuration that
 * sentence is the gateway's code itself. Giving reads a translated reason of
 * their own is the remaining half of this; it is not in this change.
 */
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
        conversation.failure === "agent-startup-deadline"
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
  // A read failure has no translated reason: `readError` is the client's text,
  // and for this one the gateway's text is its own code. See the module note.
  if (error === ConversationErrorCode.ConversationConfigurationChanged)
    return {
      title: "Conversation setup changed",
      description:
        "This chat uses a different agent configuration. Start a new conversation with the current setup.",
      retry: null,
    }
  // A control runs `create` first, so a cold agent reaches this path too, with
  // nothing sent and no failed turn to hang a draft retry on.
  if (conversation.failure === "agent-startup-deadline")
    return {
      title: "Agent was still starting",
      description: error,
      retry: { kind: "refresh" },
    }
  return {
    title: "Conversation needs attention",
    description: error,
    retry: { kind: "refresh" },
  }
}
