import type { ConversationLease } from "../generated/product.js"

/**
 * Where a conversation's lease stands, in a few words a person reads: allowed,
 * stopping, why it ended, or why it could not start. The one wording of a
 * lease for every surface, so the panel and the desktop never say two things
 * about the same lease.
 *
 * `Not known` is a lease this gateway cannot read; nothing else is claimed
 * about it.
 */
export function conversationLeaseStatus(
  lease: Pick<ConversationLease, "state" | "cause" | "refusal" | "environment">,
): string {
  switch (lease.state) {
    // Granted, not a claim that the harness is up: nothing moves a lease when
    // the agent exits on its own.
    case "live":
      return "Allowed to run"
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
        // The environment no longer held it: an SSH host's connection went
        // away, or this gateway started again.
        case "lost":
          return lease.environment === "ssh"
            ? "Ended when the connection to its host was lost"
            : "Ended when Nessa restarted"
        case "stopped":
          return "Stopped"
        // No cause recorded: ended, and nothing more is claimed.
        case undefined:
          return "Ended"
        default: {
          const exhaustive: never = lease.cause
          return exhaustive
        }
      }
    case "interrupted":
      return "Cleanup not confirmed"
    case "refused":
      switch (lease.refusal) {
        case "sandbox_unavailable":
          return "Couldn't start: sandbox not available"
        case "environment_unreachable":
          return "Couldn't start: host not reachable"
        case "environment_version_mismatch":
          return "Couldn't start: host runs another version of Nessa"
        case "environment_busy":
          return "Couldn't start: host is serving another gateway"
        case "agent_unavailable":
          return "Couldn't start: host can't run this agent"
        case "environment_platform_unsupported":
          return "Couldn't start: Nessa can't install itself on this host's system"
        case "environment_install_failed":
          return "Couldn't start: installing Nessa on the host failed"
        case undefined:
          return "Couldn't start"
        default: {
          const exhaustive: never = lease.refusal
          return exhaustive
        }
      }
    case "unreadable":
      return "Not known"
    default: {
      const exhaustive: never = lease.state
      return exhaustive
    }
  }
}

/**
 * Which machine a lease names, in a person's words: this one, or the SSH
 * destination it runs on; nothing when the lease does not say.
 */
export function conversationLeasePlace(
  lease: Pick<ConversationLease, "environment" | "host">,
): string | undefined {
  switch (lease.environment) {
    case "here":
      return "This computer"
    case "ssh":
      return lease.host
    case undefined:
      return undefined
    default: {
      const exhaustive: never = lease.environment
      return exhaustive
    }
  }
}
