/**
 * What a waiting session asks of the person, as the overview shows it: a
 * command to allow or deny, or a question only its conversation can answer.
 * The ask lives in the session's conversation, which the window may hold
 * (a pane shows it, or showed it lately) or the overview reads for itself;
 * whichever the source counted later is the one shown (`newer`).
 */
import type { WorkspaceFailureReason } from "../../../workspace/model/failure"
import type { SessionSummary } from "../../../workspace/model/overview"
import type { Approval, Transcript } from "../../../workspace/model/transcript"

export type Request =
  /** Not read yet. */
  | { readonly kind: "reading" }
  /** A command waiting on Allow or Deny. */
  | { readonly kind: "approval"; readonly approval: Approval }
  /** Waiting on a reply, which is written in the session itself. */
  | { readonly kind: "question" }
  | { readonly kind: "unreadable"; readonly reason: WorkspaceFailureReason }

/**
 * The newer of what the window holds and what the overview read, by the
 * source's count; either may be absent, and at the same count the window's.
 */
export function newer<T extends { readonly revision: number }>(
  held: T | undefined,
  read: T | undefined,
): T | undefined {
  if (!held) return read
  if (!read) return held
  return read.revision > held.revision ? read : held
}

export function requestOf(
  conversation: Pick<Transcript, "approval"> | undefined,
): Request {
  if (!conversation) return { kind: "reading" }
  return conversation.approval
    ? { kind: "approval", approval: conversation.approval }
    : { kind: "question" }
}

/**
 * How the person answered a request from the overview. `always` allows this
 * command whenever the agent asks again.
 */
export type Answer = "allow" | "always" | "deny"

/** What a settled request says in place of its buttons. */
export const answeredLabels: Readonly<Record<Answer, string>> = {
  allow: "Allowed",
  always: "Always allowed",
  deny: "Denied",
}

/** A request the person answered here, held in its place while it settles and leaves. */
export interface Settling {
  readonly sessionId: string
  readonly updatedAt: number
  /** The session and its approval as they stood when answered: what the row keeps showing. */
  readonly summary: SessionSummary
  readonly approval: Approval
  readonly answer: Answer
  /** On its way to the source; taken, saying so; fading out of the list. */
  readonly phase: "answering" | "settled" | "leaving"
}
