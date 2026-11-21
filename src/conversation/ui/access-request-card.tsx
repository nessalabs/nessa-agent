import { useEffect, useRef, useState } from "react"
import { sendChatAnswer, stepChatApproval, useChatApproval } from "./access-approvals"
import {
  atLeast,
  type ApprovalEvent,
  type AccessMode,
  type ApprovalEnding,
  type ApprovalSnapshot,
  type RaiseOutcome,
} from "../model/access-approval"
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

/**
 * The red card's whole round, for either window, drawn from one record that
 * `stepApproval` moves (`model/access-approval.ts`, whose table says every
 * ending), kept per chat for the window (`access-approvals.ts`) so a card
 * that leaves the screen and comes back finds it. The host states the chat's facts as its owners say them; this
 * judges each look at them whole, sends the person's access command and, once
 * the record says so, their Approve. The card stays drawn from the record
 * while the answer goes, so the list letting the ask go never blinks it out.
 * Renders nothing for an ordinary ask.
 */
export function AccessRequestHost<Ask extends AccessAsk>({
  chat,
  ask,
  runsAt,
  underway,
  working,
  revision,
  raise,
  answer,
}: {
  /** The host's own id for the chat: its Approve is kept under it. */
  chat: string
  /** The ask the list row carries now, of any kind. */
  ask: (Omit<Ask, "access"> & { readonly access?: Ask["access"] }) | undefined
  /** The access the chat runs at with nothing changing; absent while changing. */
  runsAt: AccessMode | undefined
  /** Where a change on its way is headed, if one is. */
  underway: AccessMode | undefined
  /** The agent is at work, so access cannot change yet. */
  working: boolean
  /** The conversation view's revision; a different one is a newer view. */
  revision: string | number | undefined
  /** Sends the person's own access command. */
  raise: (mode: "auto" | "full") => Promise<RaiseOutcome>
  /** Sends text as the person's message; resolves whether it went. */
  answer: (text: string) => Promise<boolean>
}) {
  const approval = useChatApproval<Ask>(chat)
  const dispatch = (event: ApprovalEvent<Ask>) => stepChatApproval(chat, event)
  const askId = ask?.askId
  const snapshot: ApprovalSnapshot = { askId, runsAt, underway, working, revision }
  // The look a raise's answer is judged with: the latest, when it lands.
  const latest = useRef(snapshot)
  latest.current = snapshot
  useEffect(() => {
    stepChatApproval<Ask>(chat, {
      kind: "snapshot",
      snapshot: { askId, runsAt, underway, working, revision },
    })
  }, [chat, askId, runsAt, underway, working, revision])
  // The answer goes once per record that comes due, from whichever card
  // draws it.
  useEffect(() => {
    sendChatAnswer(chat, answer)
    // `answer` is this render's; only the record starts it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chat, approval])

  const declined = useState<{ askId: string; failed: boolean } | null>(null)
  const [decline, setDecline] = declined
  const shown =
    approval?.ask ?? (ask && isAccessRequest(ask) ? (ask as unknown as Ask) : undefined)
  if (!shown) return null
  const state: AccessRequestState =
    approval?.state === "approved"
      ? "approved"
      : (approval && approval.state !== "ended") ||
          (decline?.askId === shown.askId && !decline.failed)
        ? "approving"
        : "asking"
  const held =
    approval?.state === "raising"
      ? "Changing the access…"
      : approval?.state === "waiting"
        ? working
          ? // The chat may already run at it (the source changes a running
            // turn's access); the answer still goes once the turn ends.
            atLeast(runsAt, approval.mode)
            ? "Access changed. Your answer goes when the agent stops."
            : "Access changes when the agent stops."
          : "Waiting for the access to change."
        : approval?.state === "due" || approval?.state === "answering"
          ? "Sending your answer…"
          : state === "asking" && working
            ? "You can approve once the agent stops."
            : null
  const failure =
    approval?.state === "ended"
      ? endingCopy[approval.ending]
      : decline?.askId === shown.askId && decline.failed
        ? "Not now was not sent. Press it again, or write your answer."
        : null
  return (
    <AccessRequestCard
      ask={shown}
      state={state}
      held={held}
      failure={failure}
      onApprove={() => {
        const mode = shown.access.mode
        dispatch({ kind: "approve", ask: shown, mode })
        const raised = (outcome: RaiseOutcome) =>
          dispatch({ kind: "raised", outcome, snapshot: latest.current })
        // Approve never lowers the chat's access: one already there or above
        // sends no command.
        if (atLeast(latest.current.runsAt, mode)) return raised("taken")
        raise(mode).then(raised, () => raised("refused"))
      }}
      onDecline={() => {
        const label = shown.choices[1]?.label
        if (!label) return
        setDecline({ askId: shown.askId, failed: false })
        answer(label).then(
          (ok) => setDecline({ askId: shown.askId, failed: !ok }),
          () => setDecline({ askId: shown.askId, failed: true }),
        )
      }}
    />
  )
}

const endingCopy: Record<ApprovalEnding, string> = {
  changing: "Another change to this chat is on its way. Press Approve once it settles.",
  refused: "Access didn’t change. Press Approve again, or write your answer.",
  "changed-elsewhere":
    "Access didn’t change to this. Press Approve again, or write your answer.",
  "not-sent": "Access changed, but your answer was not sent. Write it to the agent.",
}
