import * as React from "react"
import {
  AccessRequestCard,
  isAccessRequest,
  OwnerAskCard,
  type OwnerAskView,
} from "../../conversation"

/**
 * The decision an agent asked the person in the conversation on screen,
 * above the composer, while the list says it is still asked. A tapped choice
 * is sent as the person's own message, as if typed; the list then says
 * nothing is asked and the card leaves. A choice not sent offers them all
 * again.
 *
 * An agent's request for more access is the red card: Approve raises the
 * conversation's access first (`raiseAccess`, the access picker's own
 * command), then sends Approve as the answer, and the card stays to list
 * what was allowed. While the agent is at work (`working`) Approve waits.
 */
export function PanelOwnerAsk({
  ask,
  submit,
  raiseAccess,
  working,
}: {
  ask: OwnerAskView | undefined
  /** Sends text as the person's message; resolves whether the gateway took it. */
  submit: (text: string) => Promise<boolean>
  /** Raises the conversation's access; resolves whether the gateway took it. */
  raiseAccess: (mode: "auto" | "full") => Promise<boolean>
  /** The agent is at work, so access cannot change yet. */
  working: boolean
}) {
  const [sending, setSending] = React.useState<{ askId: string; label: string } | null>(
    null,
  )
  const [failure, setFailure] = React.useState<string | null>(null)
  const [approved, setApproved] = React.useState<OwnerAskView | null>(null)
  const askId = ask?.askId
  React.useEffect(() => {
    setSending(null)
    setFailure(null)
  }, [askId])
  const send = (label: string, notSent: string): Promise<boolean> => {
    const failed = () => {
      setSending(null)
      setFailure(notSent)
      return false
    }
    return submit(label).then((taken) => taken || failed(), failed)
  }
  if (approved && isAccessRequest(approved) && (!ask || ask.askId === approved.askId))
    return (
      <AccessRequestCard
        ask={approved}
        state="approved"
        held={null}
        failure={null}
        onApprove={() => {}}
        onDecline={() => {}}
      />
    )
  if (!ask) return null
  if (isAccessRequest(ask)) {
    const [approve, decline] = ask.choices
    const on = sending?.askId === ask.askId ? sending.label : null
    return (
      <AccessRequestCard
        ask={ask}
        state={on === null ? "asking" : "approving"}
        held={working ? "You can approve once the agent stops." : null}
        failure={failure}
        onApprove={() => {
          if (!approve) return
          setSending({ askId: ask.askId, label: approve.label })
          setFailure(null)
          const refused = () => {
            setSending(null)
            setFailure("Access didn’t change. Press Approve again, or write your answer.")
          }
          raiseAccess(ask.access.mode).then((raised) => {
            if (!raised) return refused()
            void send(
              approve.label,
              "Access changed, but your answer was not sent. Write it to the agent.",
            ).then((sent) => {
              if (sent) setApproved(ask)
            })
          }, refused)
        }}
        onDecline={() => {
          if (!decline) return
          setSending({ askId: ask.askId, label: decline.label })
          setFailure(null)
          void send(
            decline.label,
            "Not now was not sent. Press it again, or write your answer.",
          )
        }}
      />
    )
  }
  return (
    <OwnerAskCard
      ask={ask}
      sending={sending?.askId === ask.askId ? sending.label : null}
      failure={failure}
      onChoose={(label) => {
        setSending({ askId: ask.askId, label })
        setFailure(null)
        void send(label, "This choice was not sent. Pick it again, or write your answer.")
      }}
    />
  )
}
