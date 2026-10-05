/**
 * The words a person is shown for each reason the source gave for not doing
 * what it was asked. State holds the reason (`model/failure.ts`); this module
 * is the one place it becomes a sentence, and each table covers every reason.
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
): string {
  return readCopy[read][reason]
}
