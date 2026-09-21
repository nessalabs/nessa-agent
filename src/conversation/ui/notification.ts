import type { Conversation, ReadFailure } from "../model"

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
 * What to tell somebody whose conversation could not be refreshed.
 *
 * A read is the panel's own polling rather than anything anybody asked for, so
 * every one of these says the same two things first: the transcript is older
 * than the gateway's, and Nessa is still asking. What the reason changes is
 * whether asking again is worth anything — which is why a configuration change
 * is the one that offers no retry. Total over the vocabulary, so a word added to
 * it has to be answered here.
 */
function readNotice(reason: ReadFailure): ConversationNotice {
  switch (reason) {
    // The conversation was created against an agent configuration the gateway no
    // longer has. Reading it again asks the same question and gets the same
    // answer, so the retry is withdrawn rather than offered and disappointed.
    case "configuration-changed":
      return {
        title: "Conversation setup changed",
        description:
          "This chat uses a different agent configuration. Start a new conversation with the current setup.",
        retry: null,
      }
    case "busy":
      return {
        title: "Waiting for the gateway",
        description:
          "The gateway is not ready to answer for this conversation yet, so what is shown may be out of date. Nessa keeps asking, and this normally catches up in a moment.",
        retry: { kind: "refresh" },
      }
    // Everything else, including a cause this build has no name for. It claims
    // only what is certainly true of all of them.
    case "unavailable":
      return {
        title: "Conversation not refreshed",
        description:
          "Nessa could not read this conversation from the gateway, so what is shown may be out of date. It keeps trying.",
        retry: { kind: "refresh" },
      }
  }
}

/**
 * One notification, while individual turns retain their own delivery receipts.
 *
 * Why anything failed is asked of the panel's own words for it, which
 * `adapters/gateway/effects.ts` translated from the gateway's: `failure` for a
 * command somebody asked for, `readError` for the panel's own polling. Neither
 * is a message, and nothing here compares one against a wire code.
 *
 * A command outranks a read. It is the thing somebody was waiting on, and a read
 * that failed behind it adds only that the view is stale — which the command's
 * own sentence is already explaining.
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
  const error = conversation.remote?.permissionViewError ?? conversation.error
  if (error)
    return {
      // A control runs `create` first, so a cold agent reaches this path too,
      // with nothing sent and no failed turn to hang a draft retry on.
      title:
        conversation.failure === "agent-startup-deadline"
          ? "Agent was still starting"
          : "Conversation needs attention",
      description: error,
      retry: { kind: "refresh" },
    }
  return conversation.readError ? readNotice(conversation.readError) : null
}
