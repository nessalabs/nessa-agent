/**
 * The words a person is shown for each reason the source gave for not doing
 * what it was asked. State holds the reason (`model/failure.ts`); this table
 * is the one place it becomes a sentence, and it covers every reason.
 */
import type { WorkspaceFailureReason } from "../model/failure"

const copy: Record<WorkspaceFailureReason, string> = {
  unavailable: "Nessa couldn’t confirm this just now.",
  "unknown-session": "This session is no longer there.",
  "not-waiting": "This was already answered.",
  "not-supported": "This isn’t available for this session.",
  "signed-out": "This window isn’t signed in to the local server.",
}

export function failureCopy(reason: WorkspaceFailureReason): string {
  return copy[reason]
}
