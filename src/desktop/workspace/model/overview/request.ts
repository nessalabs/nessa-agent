/**
 * What a waiting session asks of the person, as the overview shows it: a
 * command to allow or deny, or a question only its conversation can answer.
 * The ask lives in the session's conversation, which the workspace reads and
 * keeps while the overview shows the session (`onScreen`), as it does for a
 * pane.
 */
import type { WorkspaceFailureReason } from "../failure"
import type { Approval, Transcript } from "../transcript"

export type Request =
  /** Not read yet. */
  | { readonly kind: "reading" }
  /** A command waiting on Allow or Deny. */
  | { readonly kind: "approval"; readonly approval: Approval }
  /** Waiting on a reply, which is written in the session itself. */
  | { readonly kind: "question" }
  | { readonly kind: "unreadable"; readonly reason: WorkspaceFailureReason }

/** The request in a held conversation, or why none could be read. */
export function requestOf(
  conversation: Pick<Transcript, "approval"> | undefined,
  failure: WorkspaceFailureReason | undefined,
): Request {
  if (!conversation)
    return failure ? { kind: "unreadable", reason: failure } : { kind: "reading" }
  return conversation.approval
    ? { kind: "approval", approval: conversation.approval }
    : { kind: "question" }
}
