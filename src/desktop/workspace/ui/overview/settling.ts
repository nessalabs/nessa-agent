/**
 * A request the person answered in the overview, held in its place while it
 * settles and leaves: the row keeps showing the session and its approval as
 * they stood when answered, says what became of it, then fades. This is the
 * view's hold on a row, nothing more — the answer itself is the workspace's
 * (`approve`, `deny`), and what the source does with it arrives as the
 * workspace's own updates.
 */
import type { Approval } from "../../model/transcript"
import type { SessionSummary } from "../../model/workspace-index"
import type { ApprovalChoice } from "../../model/transcript"

export interface Settling {
  readonly sessionId: string
  /** The session and its approval as they stood when answered: what the row keeps showing. */
  readonly summary: SessionSummary
  readonly approval: Approval
  readonly choice: ApprovalChoice
  /** On its way to the source; taken, saying so; fading out of the list. */
  readonly phase: "answering" | "settled" | "leaving"
}

/** What a settled request says in place of its buttons. */
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
  choice: ApprovalChoice,
  at: number,
) => void
