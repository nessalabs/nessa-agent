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
 * both of these say the same two things first: the transcript is older than the
 * gateway's, and Nessa is still asking. What the reason changes is whether
 * asking again is worth anything. Total over the vocabulary, so a word added to
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
    // The gateway could not read what it saved for this conversation, and it
    // caches that: every later read is answered from the failure without going
    // near the provider. So there is no retry to offer, exactly as above, and
    // for a firmer reason — this one it is certain about.
    case "state-unreadable":
      return {
        title: "Conversation cannot be read",
        description:
          "Nessa cannot read this conversation's saved state, so it cannot be opened again. Start a new conversation to carry on.",
        retry: null,
      }
    // Everything else, including a cause this build has no name for. It claims
    // only what is certainly true of all of them — deliberately not that the
    // gateway will come back, which no code it sends actually means.
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
 * Whether a read failure is about the conversation itself rather than about one
 * request, and so has to be said even when a command failed alongside it.
 *
 * Every other notice below ends in an action — refresh, resend, retry this
 * submission — and for these two there is no action that can now succeed. A
 * lost close acknowledgement inviting a refresh is the case that makes it
 * concrete: the refresh it asks for is exactly what is no longer possible.
 *
 * A `switch` and not a comparison against one word. `conversation.readError ===
 * "configuration-changed"` is what it was, and when `state-unreadable` joined
 * the vocabulary it compiled, the notice written for it became unreachable in
 * the only case that matters — a gateway that cannot read a conversation
 * answers every command from that same failure, so a command error always
 * coexists — and somebody got "Conversation needs attention" with a Refresh
 * that could not work. That is the regression #114 shipped to fix. Adding a
 * word to {@link ReadFailure} now does not compile until it is answered here.
 */
function outranksACommand(reason: ReadFailure): boolean {
  switch (reason) {
    case "configuration-changed":
    case "state-unreadable":
      return true
    case "unavailable":
      return false
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
 * A command otherwise outranks a read. It is the thing somebody was waiting on,
 * and a read that failed behind it adds only that the view is stale — which the
 * command's own sentence is already explaining.
 */
export function conversationNotice(
  conversation: Conversation,
): ConversationNotice | null {
  // Except for the read failures that are about the conversation rather than
  // about a request; see `outranksACommand`. What became of each message is not
  // lost with the slot: the turn keeps its own receipt, and the transcript
  // still shows it.
  if (conversation.readError && outranksACommand(conversation.readError))
    return readNotice(conversation.readError)
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
  const startupFailure = conversation.remote?.lifecycle.failure
  if (startupFailure)
    return {
      title: "Agent could not start",
      description: startupFailure.message,
      retry: { kind: "refresh" },
    }
  const evidenceFailure = conversation.remote?.lifecycle.evidenceFailure
  if (evidenceFailure)
    return {
      title: "Agent lifecycle record failed",
      description: evidenceFailure.message,
      retry: { kind: "refresh" },
    }
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
