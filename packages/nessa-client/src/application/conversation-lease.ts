import type { ConversationLease } from "../generated/product.js"

/**
 * Where a conversation's lease stands, in a few words a person reads: running,
 * stopping, why it ended, or why it could not start. The one wording of a
 * lease for every surface, so the panel and the desktop never say two things
 * about the same lease.
 *
 * `Not known` is a lease this gateway cannot read; nothing else is claimed
 * about it.
 */
export function conversationLeaseStatus(
  lease: Pick<ConversationLease, "state" | "cause" | "refusal">,
): string {
  switch (lease.state) {
    case "live":
      return "Running"
    case "ending":
      return "Stopping"
    case "ended":
      switch (lease.cause) {
        case "closed":
          return "Closed"
        case "revoked":
          return "Access withdrawn"
        case "expired":
          return "Timed out"
        case "lost":
          return "Ended when Nessa restarted"
        case "stopped":
        case undefined:
          return "Stopped"
        default: {
          const exhaustive: never = lease.cause
          return exhaustive
        }
      }
    case "interrupted":
      return "Cleanup not confirmed"
    case "refused":
      return lease.refusal === "sandbox_unavailable"
        ? "Couldn't start: sandbox not available"
        : "Couldn't start"
    case "unreadable":
      return "Not known"
    default: {
      const exhaustive: never = lease.state
      return exhaustive
    }
  }
}
