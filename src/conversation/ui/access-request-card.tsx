import { useEffect, useState } from "react"
import { AccessRequest, type AccessRequestState } from "@nessa-ui/react/access-request"

/** What the card reads of an access request, as either window holds one. */
interface AccessAsk {
  readonly askId: string
  readonly question: string
  readonly reason: string
  readonly choices: readonly { readonly label: string }[]
  readonly access: {
    readonly mode: "auto" | "full"
    readonly grants: readonly string[]
  }
}

/**
 * An agent's request to raise the chat's access (`request_access`), drawn as
 * the red card: Nessa's question, the agent's reason, Approve and Not now,
 * and once approved the lines saying what the person allowed. Every word but
 * the reason is the gateway's; the card only draws them. The host raises the
 * access on Approve, by the person's own access command, and then sends the
 * Approve label as their answer.
 */
export function AccessRequestCard({
  ask,
  state,
  held,
  failure,
  onApprove,
  onDecline,
}: {
  ask: AccessAsk
  state: AccessRequestState
  /** Why Approve can't be pressed now, in words. */
  held: string | null
  /** Why the last Approve or Not now did not go, in words. */
  failure: string | null
  onApprove: () => void
  onDecline: () => void
}) {
  const [approve, decline] = ask.choices
  return (
    <AccessRequest
      className="conversation-access-request"
      data-owner-ask={ask.askId}
      data-access-request={ask.access.mode}
      title={ask.question}
      reason={ask.reason}
      grants={ask.access.grants}
      state={state}
      {...(held ? { held } : {})}
      {...(failure ? { failure } : {})}
      {...(approve ? { approveLabel: approve.label } : {})}
      {...(decline ? { declineLabel: decline.label } : {})}
      onApprove={onApprove}
      onDecline={onDecline}
    />
  )
}

/** Whether an ask is an access request, drawn as the red card. */
export function isAccessRequest<Ask extends { readonly access?: AccessAsk["access"] }>(
  ask: Ask,
): ask is Ask & { readonly access: AccessAsk["access"] } {
  return ask.access !== undefined
}

type AccessMode = "ask" | "auto" | "full"

/**
 * What became of the person's access command, as the window that sent it
 * says: `taken` by the gateway, `pending` until the turn at work ends (still
 * the person's command), `changing` because another change is on its way,
 * or `refused`.
 */
export type RaiseOutcome = "taken" | "pending" | "changing" | "refused"

/** Where the card stands, for the ask it is about. */
type Phase<Ask> =
  | { readonly kind: "asking" }
  /** Approve pressed: the person's access command is on its way. */
  | { readonly kind: "raising"; readonly ask: Ask }
  /** Taken: the answer waits until the chat runs at the access. */
  | { readonly kind: "due"; readonly ask: Ask }
  /** Sent, or being sent: the card lists what was allowed. */
  | { readonly kind: "approved"; readonly ask: Ask }

/**
 * The red card's whole round, for either window: one owner, so the desktop
 * window and the panel cannot drift apart on when Approve's answer goes.
 *
 * Approve sends the person's own access command (`raise`) and the Approve
 * label (`answer`) only once the chat runs at that access (`runsAt`), the
 * end state and never the command's reply. The wait ends without an answer
 * when the ask leaves (the person answered in words) or when the access is
 * headed somewhere else (`heading`, the person picked another). The card
 * turns to what was allowed as the answer goes, so it never blinks out while
 * the list lets the ask go, and leaves when a new ask comes or the chat no
 * longer runs at that access. Renders nothing for an ordinary ask.
 */
export function AccessRequestHost<Ask extends AccessAsk>({
  ask,
  runsAt,
  heading,
  working,
  raise,
  answer,
}: {
  /** The ask the list row carries now, of any kind. */
  ask: (Omit<Ask, "access"> & { readonly access?: Ask["access"] }) | undefined
  /**
   * The access the chat runs at with nothing changing, as its conversation
   * last said; absent while any change (of access or model) is on its way.
   */
  runsAt: AccessMode | undefined
  /**
   * Where the chat's access is headed, as far as its window knows: the
   * access a change on its way asks for, or where a refused one leaves it.
   * Absent when nothing is under way.
   */
  heading: AccessMode | undefined
  /** The agent is at work, so access cannot change yet. */
  working: boolean
  /** Sends the person's own access command. */
  raise: (mode: "auto" | "full") => Promise<RaiseOutcome>
  /** Sends text as the person's message; resolves whether it went. */
  answer: (text: string) => Promise<boolean>
}) {
  const [phase, setPhase] = useState<Phase<Ask>>({ kind: "asking" })
  const [failure, setFailure] = useState<string | null>(null)
  const askId = ask?.askId
  // A new ask, or none while the answer is not on its way yet, starts over;
  // the approved card goes once another ask comes.
  useEffect(() => {
    setPhase((now) => {
      if (now.kind === "asking") return now
      if (now.ask.askId === askId) return now
      if (now.kind === "approved" && askId === undefined) return now
      return { kind: "asking" }
    })
    setFailure(null)
  }, [askId])
  const due = phase.kind === "due" ? phase.ask : undefined
  const mode = due?.access.mode
  useEffect(() => {
    if (!due || mode === undefined) return
    if (heading !== undefined && heading !== mode) {
      // Refused, or the person chose another access since: this Approve
      // no longer ends at what it asked for.
      setPhase({ kind: "asking" })
      setFailure(
        "Access didn’t change to this. Press Approve again, or write your answer.",
      )
      return
    }
    if (working || runsAt !== mode) return
    setPhase({ kind: "approved", ask: due })
    const label = due.choices[0]?.label
    if (!label) return
    void answer(label).then(
      (sent) => sent || notSent(),
      () => notSent(),
    )
    function notSent() {
      setPhase({ kind: "asking" })
      setFailure("Access changed, but your answer was not sent. Write it to the agent.")
      return false
    }
    // `answer` is this render's; only the due ask and the chat's access start it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [due, mode, heading, working, runsAt])

  if (phase.kind === "approved") {
    if (ask && ask.askId !== phase.ask.askId) return null
    if (runsAt !== undefined && runsAt !== phase.ask.access.mode) return null
    return (
      <AccessRequestCard
        ask={phase.ask}
        state="approved"
        held={null}
        failure={null}
        onApprove={() => {}}
        onDecline={() => {}}
      />
    )
  }
  if (phase.kind !== "asking")
    return (
      <AccessRequestCard
        ask={phase.ask}
        state="approving"
        held={null}
        failure={null}
        onApprove={() => {}}
        onDecline={() => {}}
      />
    )
  if (!ask || !isAccessRequest(ask)) return null
  const shown = ask as unknown as Ask
  const [approve, decline] = shown.choices
  return (
    <AccessRequestCard
      ask={shown}
      state="asking"
      held={working ? "You can approve once the agent stops." : null}
      failure={failure}
      onApprove={() => {
        if (!approve) return
        setPhase({ kind: "raising", ask: shown })
        setFailure(null)
        const refused = (outcome: RaiseOutcome | null) => {
          setPhase({ kind: "asking" })
          setFailure(
            outcome === "changing"
              ? "Access is still changing. Press Approve once it settles."
              : "Access didn’t change. Press Approve again, or write your answer.",
          )
        }
        raise(shown.access.mode).then(
          (outcome) => {
            if (outcome === "taken" || outcome === "pending")
              setPhase((now) =>
                now.kind === "raising" && now.ask.askId === shown.askId
                  ? { kind: "due", ask: shown }
                  : now,
              )
            else refused(outcome)
          },
          () => refused(null),
        )
      }}
      onDecline={() => {
        if (!decline) return
        setPhase({ kind: "raising", ask: shown })
        setFailure(null)
        void answer(decline.label).then(
          (sent) => {
            setPhase({ kind: "asking" })
            if (!sent)
              setFailure("Not now was not sent. Press it again, or write your answer.")
          },
          () => {
            setPhase({ kind: "asking" })
            setFailure("Not now was not sent. Press it again, or write your answer.")
          },
        )
      }}
    />
  )
}
