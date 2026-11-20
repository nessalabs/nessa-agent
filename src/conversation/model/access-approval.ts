/**
 * The person's Approve on an agent's access request, from the press to the
 * answer: one record, and one table that moves it. Both windows draw the red
 * card from it; neither decides an ending of its own.
 *
 * Approve sends the person's own access command, and the Approve label goes
 * as their message only once the chat runs at that access. Every ending is
 * judged on one snapshot of what the chat's owners say — the ask on the list
 * row, the access the conversation runs at, a change on its way, a turn at
 * work, and the view's revision — and the ask is always judged first, so an
 * ask that left and an access that arrived in the same snapshot send nothing.
 *
 * | State    | Event                                              | Next                    |
 * | -------- | -------------------------------------------------- | ----------------------- |
 * | raising  | raised: taken or pending                           | waiting, at this view   |
 * | raising  | raised: changing or refused                        | ended, saying which     |
 * | raising  | snapshot: the row's ask is not this one            | gone, nothing sent      |
 * | waiting  | snapshot: the row's ask is not this one            | gone, nothing sent      |
 * | waiting  | snapshot: a change on its way to this access, or   | waiting                 |
 * |          | a turn at work                                     |                         |
 * | waiting  | snapshot: a change on its way to another access    | ended: didn't change    |
 * | waiting  | snapshot: runs at this access                      | sending                 |
 * | waiting  | snapshot: runs at another, in the view of the raise | waiting (not newer)    |
 * | waiting  | snapshot: runs at another, in a newer view         | ended: didn't change    |
 * | sending  | snapshot: anything (the ask leaving is our answer) | sending                 |
 * | sending  | answered: sent                                     | approved                |
 * | sending  | answered: not sent                                 | ended: not sent         |
 * | approved | snapshot: another ask, or runs at another access   | gone                    |
 * | ended    | snapshot: the row's ask is not this one            | gone                    |
 * | ended    | Approve pressed again                              | raising                 |
 */

export type AccessMode = "ask" | "auto" | "full"

/** What became of the person's access command, as their window says. */
export type RaiseOutcome = "taken" | "pending" | "changing" | "refused"

/** Why a wait ended without an answer, in the card's words. */
export type ApprovalEnding = "changing" | "refused" | "changed-elsewhere" | "not-sent"

/** One look at the chat, each fact from its owner, taken at once. */
export interface ApprovalSnapshot {
  /** The ask the list row carries now, if any. */
  readonly askId: string | undefined
  /** The access the chat runs at with nothing changing; absent while changing. */
  readonly runsAt: AccessMode | undefined
  /** Where a change on its way is headed, if one is. */
  readonly underway: AccessMode | undefined
  /** A turn is at work, so access changes only after it. */
  readonly working: boolean
  /** The conversation view's revision: a different one is a newer view. */
  readonly revision: string | number | undefined
}

/** The record of one Approve, for one ask. */
export type AccessApproval<Ask> =
  | { readonly state: "raising"; readonly ask: Ask; readonly mode: AccessMode }
  | {
      readonly state: "waiting"
      readonly ask: Ask
      readonly mode: AccessMode
      /** The view the raise was taken at: only a newer one can end the wait. */
      readonly takenAt: string | number | undefined
    }
  | { readonly state: "sending"; readonly ask: Ask; readonly mode: AccessMode }
  | { readonly state: "approved"; readonly ask: Ask; readonly mode: AccessMode }
  | {
      readonly state: "ended"
      readonly ask: Ask
      readonly mode: AccessMode
      readonly ending: ApprovalEnding
    }

export type ApprovalEvent<Ask> =
  | { readonly kind: "approve"; readonly ask: Ask; readonly mode: AccessMode }
  | {
      readonly kind: "raised"
      readonly outcome: RaiseOutcome
      readonly snapshot: ApprovalSnapshot
    }
  | { readonly kind: "snapshot"; readonly snapshot: ApprovalSnapshot }
  | { readonly kind: "answered"; readonly sent: boolean }

/** The record after `event`; `undefined` is no record: the card draws the row's ask. */
export function stepApproval<Ask extends { readonly askId: string }>(
  approval: AccessApproval<Ask> | undefined,
  event: ApprovalEvent<Ask>,
): AccessApproval<Ask> | undefined {
  if (event.kind === "approve") {
    if (approval && approval.state !== "ended") return approval
    return { state: "raising", ask: event.ask, mode: event.mode }
  }
  if (!approval) return approval
  const { ask, mode } = approval
  switch (event.kind) {
    case "raised":
      if (approval.state !== "raising") return approval
      if (event.snapshot.askId !== ask.askId) return undefined
      return event.outcome === "taken" || event.outcome === "pending"
        ? { state: "waiting", ask, mode, takenAt: event.snapshot.revision }
        : { state: "ended", ask, mode, ending: event.outcome }
    case "answered":
      if (approval.state !== "sending") return approval
      return event.sent
        ? { state: "approved", ask, mode }
        : { state: "ended", ask, mode, ending: "not-sent" }
    case "snapshot": {
      const now = event.snapshot
      switch (approval.state) {
        case "sending":
          return approval
        case "approved":
          if (now.askId !== undefined && now.askId !== ask.askId) return undefined
          if (now.runsAt !== undefined && now.runsAt !== mode) return undefined
          return approval
        case "raising":
        case "ended":
          return now.askId === ask.askId ? approval : undefined
        case "waiting":
          if (now.askId !== ask.askId) return undefined
          if (now.underway !== undefined)
            return now.underway === mode
              ? approval
              : { state: "ended", ask, mode, ending: "changed-elsewhere" }
          if (now.working) return approval
          if (now.runsAt === mode) return { state: "sending", ask, mode }
          if (now.runsAt === undefined || now.revision === approval.takenAt)
            return approval
          return { state: "ended", ask, mode, ending: "changed-elsewhere" }
      }
    }
  }
}
