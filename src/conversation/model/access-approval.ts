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
 * "At" an access means that access or more (Full is more than Auto, Auto
 * than Ask), as the gateway's own `held_at` counts it: Approve never lowers
 * the chat's access, and a chat already there or above sends no command.
 *
 * | State     | Event                                               | Next                  |
 * | --------- | --------------------------------------------------- | --------------------- |
 * | (none)    | Approve, chat already at the access                 | raising, then raised  |
 * |           |                                                     | taken, no command     |
 * | raising   | raised: taken or pending                            | waiting, at this view,|
 * |           |                                                     | then that look judged |
 * |           |                                                     | as waiting is         |
 * | raising   | raised: changing or refused                         | ended, saying which   |
 * | raising   | snapshot or raised: the row's ask is not this one   | gone, nothing sent    |
 * | waiting   | snapshot: the row's ask is not this one             | gone, nothing sent    |
 * | waiting   | snapshot: a change on its way to at least this      | waiting               |
 * |           | access, or a turn at work                           |                       |
 * | waiting   | snapshot: a change on its way to less               | ended: didn't change  |
 * | waiting   | snapshot: runs at this access                       | due                   |
 * | waiting   | snapshot: access not known, no change named         | waiting               |
 * | waiting   | snapshot: runs at less, in the view of the raise    | waiting (not newer)   |
 * | waiting   | snapshot: runs at less, in a newer view             | ended: didn't change  |
 * | due       | snapshot: the row's ask is not this one             | gone, nothing sent    |
 * | due       | snapshot: anything else                             | due                   |
 * | due       | answering: a card starts the answer                 | answering             |
 * | answering | snapshot: anything (the ask leaving is this answer) | answering             |
 * | answering | answered: sent / not sent                           | approved / ended:     |
 * |           |                                                     | not sent              |
 * | approved  | snapshot: no ask or this one, at this access        | approved              |
 * | approved  | snapshot: another ask, or runs at less              | gone                  |
 * | ended     | snapshot: the row's ask is not this one             | gone                  |
 * | ended     | Approve pressed again                               | raising               |
 */

export type AccessMode = "ask" | "auto" | "full"

const rank: Record<AccessMode, number> = { ask: 0, auto: 1, full: 2 }

/** Whether `now` is `mode` or more, as the gateway's `held_at` counts it. */
export function atLeast(now: AccessMode | undefined, mode: AccessMode): boolean {
  return now !== undefined && rank[now] >= rank[mode]
}

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
  /** At the access: the answer goes from the next card that draws it. */
  | { readonly state: "due"; readonly ask: Ask; readonly mode: AccessMode }
  /** A card started the answer: the ask leaving is that answer landing. */
  | { readonly state: "answering"; readonly ask: Ask; readonly mode: AccessMode }
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
  | { readonly kind: "answering" }
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
      if (event.outcome !== "taken" && event.outcome !== "pending")
        return { state: "ended", ask, mode, ending: event.outcome }
      // The look the raise landed in is judged too: its view may already
      // run at the access (the window read it before the command returned,
      // or the chat was already there), and no later look need come.
      return stepApproval<Ask>(
        { state: "waiting", ask, mode, takenAt: event.snapshot.revision },
        { kind: "snapshot", snapshot: event.snapshot },
      )
    case "answering":
      return approval.state === "due" ? { state: "answering", ask, mode } : approval
    case "answered":
      if (approval.state !== "answering") return approval
      return event.sent
        ? { state: "approved", ask, mode }
        : { state: "ended", ask, mode, ending: "not-sent" }
    case "snapshot": {
      const now = event.snapshot
      switch (approval.state) {
        case "answering":
          return approval
        case "approved":
          if (now.askId !== undefined && now.askId !== ask.askId) return undefined
          if (now.runsAt !== undefined && !atLeast(now.runsAt, mode)) return undefined
          return approval

        case "raising":
        case "due":
        case "ended":
          return now.askId === ask.askId ? approval : undefined
        case "waiting":
          if (now.askId !== ask.askId) return undefined
          if (now.underway !== undefined)
            return atLeast(now.underway, mode)
              ? approval
              : { state: "ended", ask, mode, ending: "changed-elsewhere" }
          if (now.working) return approval
          if (atLeast(now.runsAt, mode)) return { state: "due", ask, mode }
          if (now.runsAt === undefined || now.revision === approval.takenAt)
            return approval
          return { state: "ended", ask, mode, ending: "changed-elsewhere" }
      }
    }
  }
}
