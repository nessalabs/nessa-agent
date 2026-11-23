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
  "agent-not-configured": "This agent needs setup.",
  "not-supported": "This isn’t available for this session.",
  "signed-out": "This window isn’t signed in to the local server.",
  "not-started": startupRefusalSentence("not-provisioned"),
  "not-ready": startupRefusalSentence("not-ready"),
  "still-starting": startupRefusalSentence("still-starting"),
  "not-listening": startupRefusalSentence("not-listening"),
  "wrong-stage": wrongStageSentence(null),
  "source-gone":
    "This conversation’s recorded version is no longer at its source. Other conversations are unchanged.",
  "source-changed":
    "This conversation’s recorded version changed at its source, so it was not restored. Other conversations are unchanged.",
}

export function failureCopy(
  reason: WorkspaceFailureReason,
  stages?: StageMismatch | null,
): string {
  if (reason === "wrong-stage") return wrongStageSentence(stages)
  return copy[reason]
}

/**
 * What a failed read was of. `index` is the local server's conversations
 * (`EmptyWorkspace`). `conversation` is this one — the transcript, the peek,
 * and a request row's unreadable (#433).
 */
export type ReadSubject = "index" | "conversation"

/**
 * The words for a read the window could not finish: the same as any call's,
 * except where no answer came, which names what could not be read rather
 * than that a call is unconfirmed
 * (`a read says what it could not read only where no answer came`).
 */
const readCopy: Record<ReadSubject, Record<WorkspaceFailureReason, string>> = {
  index: {
    ...copy,
    unavailable: "Nessa couldn’t read the local server’s conversations just now.",
  },
  conversation: {
    ...copy,
    unavailable: "Nessa couldn’t read this conversation just now.",
  },
}

export function readFailureCopy(
  reason: WorkspaceFailureReason,
  read: ReadSubject,
  stages?: StageMismatch | null,
): string {
  if (reason === "wrong-stage") return wrongStageSentence(stages)
  return readCopy[read][reason]
}

/**
 * Two or three words for why a message did not go, said right under it
 * (`UnsentRow`). The whole reason is behind its copy button, never here:
 * `setupFailed` when its cloud machine did not start, else the gateway's
 * code where it names one a person can act on, else the reason.
 */
export function unsentLabel(
  reason: WorkspaceFailureReason,
  code: string | undefined,
  setupFailed: boolean,
): string {
  // A cause the person can fix themselves is named first.
  switch (code) {
    case "agent_sign_in_required":
      return "Not signed in"
    case "environment_not_configured":
      return "Boat not connected"
  }
  if (setupFailed) return "Container didn’t start"
  switch (code) {
    case "environment_unavailable":
    case "environment_unreachable":
      return "Machine unreachable"
  }
  switch (reason) {
    case "agent-not-configured":
      return "Agent needs setup"
    case "signed-out":
      return "Not signed in"
    default:
      return "Couldn’t send"
  }
}
