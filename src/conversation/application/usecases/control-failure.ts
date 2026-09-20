import type { CommandFailure } from "../../model"

/**
 * What to tell somebody whose conversation control the gateway could not run.
 *
 * The client has no sentence of its own for a control. Every one of them gets
 * the same constant — "Conversation control did not return a trustworthy
 * acknowledgement" — which names neither the command nor its cause, and is
 * true only when the control really may have been applied. So this answers
 * undefined exactly where that constant is honest, and says something itself
 * where it is not.
 *
 * `refused` is the client's verdict that nothing was applied, and is what
 * licenses a sentence to say so. The reason alone cannot: the same reason can
 * arrive either way, and one of them — a close whose cleanup failed — names a
 * control that certainly did run.
 *
 * A refused *message* is the other half of this vocabulary and says its own
 * sentences in `send-draft.ts`. Same reasons, different news: a control has no
 * draft to hand back.
 */
export function controlFailureMessage(
  reason: CommandFailure,
  refused: boolean,
): string | undefined {
  switch (reason) {
    // The one code that is not a refusal at all, and the only one whose news
    // the gateway carries in the code itself: the protocol defines it as a
    // close that did happen whose release of this conversation's uploads did
    // not, and the gateway raises it from nowhere else. Nothing in this panel
    // depends on those files — a draft's stored images are forgotten after any
    // close, acknowledged or not — so this reports rather than asks.
    case "attachment-cleanup-unavailable":
      return "The conversation stopped, but the gateway could not release the images it had stored for it. Nothing here depends on them; what the gateway still holds is its own to account for."
    // Both of these the gateway decides before touching the conversation, so
    // when the client says the control was refused, "nothing happened" is a
    // fact worth stating rather than leaving as an untrustworthy answer. When
    // it does not say so, this has nothing the client's constant does not.
    case "agent-startup-deadline":
      return refused
        ? "The agent was still starting and ran out of time, so nothing was done. Starting it is slowest the first time after an install or update; once the runtime is warm this normally works."
        : undefined
    case "invalid-request":
      return refused
        ? "The gateway would not accept this action, so nothing was done."
        : undefined
    // Everything else leaves the outcome open, which is precisely what the
    // client's own sentence says.
    case "image-input-unsupported":
    case "attachment-not-found":
    case "attachment-unavailable":
    case "conversation-not-found":
    case "conversation-capacity":
    case "agent-not-configured":
      return undefined
  }
}
