import {
  failureReason,
  WorkspaceSourceError,
  type WorkspaceFailureReason,
} from "../../application/ports"

/**
 * Why a call to the source failed, by its type. Anything but the source's own
 * typed refusal is a fault, not an answer: it is logged as one, and shown as
 * unreachable. Commands and effects both answer failures through this.
 */
export function refusal(error: unknown): WorkspaceFailureReason {
  if (!(error instanceof WorkspaceSourceError))
    console.error("The workspace source failed unexpectedly", error)
  return failureReason(error)
}
