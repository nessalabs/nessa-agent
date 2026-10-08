/**
 * A request the person answered in the overview, held in its place while it
 * leaves: the row keeps showing the session and its approval as they stood
 * when answered; taken, it leaves its place as the session arrives in its
 * new one, in one beat (`leaveInPlace`). What became of it is said to a
 * screen reader (the overview's live region), and in the peek beside the
 * list while it is still shown. This is the view's hold on a row, nothing
 * more — the answer itself is the workspace's (`approve`, `deny`), and what
 * the source does with it arrives as the workspace's own updates.
 *
 * The orderings it is held through (`overview.tsx`, the answer's `.then`):
 *
 * | phase      | event                                     | then                                  |
 * | ---------- | ----------------------------------------- | ------------------------------------- |
 * | answering  | answer refused, unconfirmed, or not asked | released: the row as the workspace has it |
 * | answering  | sent, the session already moved on        | leaving: copy left in place, released at once |
 * | answering  | sent, the session still waits             | leaving: held, watching the store     |
 * | leaving    | the source moves the session on           | copy left in place, released          |
 * | leaving    | the source asks something new             | released; the row stays, asking again |
 * | leaving    | `heldAtMost` passes with no move          | released as the workspace has it      |
 * | any        | the overview closes                       | watches and timers stopped; a late answer does nothing |
 * | any        | reduced motion                            | no copy; the row is simply where it now belongs |
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
