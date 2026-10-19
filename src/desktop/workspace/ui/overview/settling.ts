/**
 * The view's hold on an answered row while the workspace source catches up.
 * Session, approval and choice stay together until released; source-delay,
 * refusal and cleanup orderings are owned by ADR 238's "Overview list
 * placement and answered requests" table. The answer itself belongs to
 * the workspace's approve/deny commands.
 */
import type { Approval, ApprovalChoice, ApprovalOption } from "../../model/transcript"
import type { SessionSummary } from "../../model/workspace-index"

export interface Settling {
  readonly sessionId: string
  /** The session and its approval as they stood when answered: what the row keeps showing. */
  readonly summary: SessionSummary
  readonly approval: Approval
  readonly choice: ApprovalChoice
  /** On its way to the source; taken, and leaving the list. */
  readonly phase: "answering" | "leaving"
}

/** What became of an answered request: said to a screen reader, and in the peek. */
export const answeredLabels: Readonly<Record<ApprovalChoice, string>> = {
  once: "Allowed",
  always: "Always allowed",
  deny: "Denied",
}

/**
 * Answers an approval shown in the overview, as the person. `at` is when the
 * answer was made — the key's or click's `timeStamp`, on `performance.now()`'s
 * clock — from which the next press is timed (`takesAnswerKey`).
 */
export type OnAnswer = (
  summary: SessionSummary,
  approval: Approval,
  option: ApprovalOption,
  at: number,
) => void
