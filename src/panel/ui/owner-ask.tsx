import * as React from "react"
import { OwnerAskCard, type OwnerAskView } from "../../conversation"

/**
 * The decision an agent asked the person in the conversation on screen,
 * above the composer, while the list says it is still asked. A tapped choice
 * is sent as the person's own message, as if typed; the list then says
 * nothing is asked and the card leaves. A choice not sent offers them all
 * again.
 */
export function PanelOwnerAsk({
  ask,
  submit,
}: {
  ask: OwnerAskView | undefined
  /** Sends text as the person's message; resolves whether the gateway took it. */
  submit: (text: string) => Promise<boolean>
}) {
  const [sending, setSending] = React.useState<{ askId: string; label: string } | null>(
    null,
  )
  const [failure, setFailure] = React.useState<string | null>(null)
  const askId = ask?.askId
  React.useEffect(() => {
    setSending(null)
    setFailure(null)
  }, [askId])
  if (!ask) return null
  const notSent = () => {
    setSending(null)
    setFailure("This choice was not sent. Pick it again, or write your answer.")
  }
  return (
    <OwnerAskCard
      ask={ask}
      sending={sending?.askId === ask.askId ? sending.label : null}
      failure={failure}
      onChoose={(label) => {
        setSending({ askId: ask.askId, label })
        setFailure(null)
        submit(label).then((taken) => {
          if (!taken) notSent()
        }, notSent)
      }}
    />
  )
}
