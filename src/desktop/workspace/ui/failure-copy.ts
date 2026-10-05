/**
 * The words a person is shown for each reason the source gave for not doing
 * what it was asked. State holds the reason (`model/failure.ts`); this module
 * is the one place it becomes a sentence, and each table covers every reason.
 */
import {
  startupRefusalSentence,
  wrongStageSentence,
  type StageMismatch,
} from "../../../host/startup-refusals"
import type { WorkspaceFailureReason } from "../model/failure"

const copy: Record<WorkspaceFailureReason, string> = {
  unavailable: "Nessa couldn’t confirm this just now.",
  "unknown-session": "This session is no longer there.",
  "not-waiting": "This was already answered.",
  "not-supported": "This isn’t available for this session.",
  "signed-out": "This window isn’t signed in to the local server.",
  "not-started": startupRefusalSentence("not-provisioned"),
  "not-ready": startupRefusalSentence("not-ready"),
  "not-listening": startupRefusalSentence("not-listening"),
  "wrong-stage": wrongStageSentence(null),
}

export function failureCopy(
  reason: WorkspaceFailureReason,
  stages?: StageMismatch | null,
): string {
  if (reason === "wrong-stage") return wrongStageSentence(stages)
  return copy[reason]
}

/**
 * The words for an index the window could not read (`EmptyWorkspace`): the
 * same as any call's, except where no answer came, which names what could
 * not be read and from where rather than that a call is unconfirmed.
 */
const readCopy: Record<WorkspaceFailureReason, string> = {
  ...copy,
  unavailable: "Nessa couldn’t read the local server’s conversations just now.",
}

export function readFailureCopy(
  reason: WorkspaceFailureReason,
  stages?: StageMismatch | null,
): string {
  if (reason === "wrong-stage") return wrongStageSentence(stages)
  return readCopy[reason]
}
