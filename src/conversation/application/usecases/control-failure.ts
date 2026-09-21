import type { CommandFailure } from "../../model"
import type { ControlOutcome } from "../ports"

/**
 * What to tell somebody whose conversation control the gateway could not run.
 *
 * The client has no sentence of its own for a control. Every one of them gets
 * the same constant — "Conversation control did not return a trustworthy
 * acknowledgement" — which names neither the command nor its cause, and claims
 * the outcome is unknown. So the outcome is what decides whether the panel
 * speaks: the constant stands where it is true, and where the gateway said
 * otherwise this contradicts it rather than repeating it.
 *
 * The reason only shapes the sentence; it never licenses one. A review the
 * gateway reports as still pending is certainly not applied under any code,
 * including one this build has no word for, and that case gets the same plain
 * "nothing was done" as the rest. Saying so is not reading an unknown code as a
 * known one: what happened came from the gateway's own verdict, and the reason
 * stays untyped.
 *
 * A refused *message* is the other half of this vocabulary and says its own
 * sentences in `send-draft.ts`. Same reasons, different news: a control has no
 * draft to hand back.
 */
export function controlFailureMessage(
  reason: CommandFailure | undefined,
  outcome: ControlOutcome,
): string | undefined {
  // The one code whose news is the code itself, whatever became of the rest of
  // the command: the protocol defines it as a close that did happen whose
  // release of this conversation's uploads did not, and the gateway raises it
  // from nowhere else. Nothing in this panel depends on those files — a draft's
  // stored images are forgotten after any close, acknowledged or not — so this
  // reports rather than asks.
  if (reason === "attachment-cleanup-unavailable")
    return "The conversation stopped, but the gateway could not release the images it had stored for it. Nothing here depends on them; what the gateway still holds is its own to account for."
  switch (outcome) {
    // Exactly what the client's constant says, so there is nothing to add.
    case "unknown":
      return undefined
    // Only a permission answer can reach this: the gateway reported the
    // reviewed option as consumed, which it calls authoritative. The choice
    // took effect and the command failed after it, so asking for it again
    // would be asking somebody to decide something already decided.
    case "applied":
      return "The gateway recorded your choice before the request failed, so there is nothing to answer again. The conversation has been read again to show where it stands."
    case "refused":
      return nothingWasDone(reason)
  }
}

/**
 * The gateway decided this control without applying any of it. Said plainly,
 * with whatever the reason adds — and said anyway when it adds nothing, because
 * "nothing was done" is the part that matters and it does not come from the
 * code. Total over the vocabulary, so a reason added to it has to be answered
 * here rather than quietly collapsing into the general case.
 */
function nothingWasDone(
  reason: Exclude<CommandFailure, "attachment-cleanup-unavailable"> | undefined,
): string {
  switch (reason) {
    case "agent-startup-deadline":
      return "The agent was still starting and ran out of time, so nothing was done. Starting it is slowest the first time after an install or update; once the runtime is warm this normally works."
    case "agent-not-configured":
      return "The gateway has no agent configured, so nothing was done. Configure one and restart the gateway."
    case "agent-unsupported":
      return "This conversation runs on an agent this gateway cannot start, so nothing was done. Configure that agent and restart the gateway."
    case "conversations-not-configured":
      return "This gateway is not set up to run conversations, so nothing was done. Configure an agent and restart it."
    case "conversation-not-found":
      return "The gateway no longer has this conversation, so nothing was done."
    case "conversation-capacity":
      return "The gateway has too many conversations open, so nothing was done. Close one and try again shortly."
    // Nothing these add is worth a sentence to somebody who pressed a control:
    // the image codes describe a message, `invalid-request` says only what the
    // sentence already says, and an undefined reason is a code with no word
    // here at all. What they share is the part that matters.
    case "image-input-unsupported":
    case "attachment-not-found":
    case "attachment-unavailable":
    case "invalid-request":
    case undefined:
      return "The gateway would not take this action, so nothing was done."
  }
}
