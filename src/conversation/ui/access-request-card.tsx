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
