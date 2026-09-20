import type { CommandFailure } from "../../model"

/**
 * What to tell somebody whose conversation control the gateway could not
 * complete, chosen by its typed reason.
 *
 * Only the failures a control has something of its own to say about. Everything
 * else answers undefined and the client's own sentence is shown instead, which
 * is what the panel has always done for a control: it names the command and its
 * cause better than a word here could. A refused *message* is the other half of
 * this, and says its own sentences in `send-draft.ts` — the same reasons, but
 * not the same news, because a control has no draft to hand back.
 */
export function controlFailureMessage(reason: CommandFailure): string | undefined {
  switch (reason) {
    // The one code that is not a refusal: the conversation did close, and only
    // letting go of the files it held did not. Nothing in this panel depends on
    // them — a draft's stored images are forgotten on any close, answered or
    // not — so this says what happened rather than asking for anything.
    case "attachment-cleanup-unavailable":
      return "The conversation stopped, but the gateway could not release the images it had stored for it. Nothing here depends on them; what the gateway still holds is its own to account for."
    case "image-input-unsupported":
    case "attachment-not-found":
    case "attachment-unavailable":
    case "conversation-not-found":
    case "conversation-capacity":
    case "agent-not-configured":
    case "agent-startup-deadline":
    case "invalid-request":
      return undefined
  }
}
