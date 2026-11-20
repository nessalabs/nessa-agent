import { useEffect, useState } from "react"
import { OwnerAskCard } from "../../../../conversation"
import { sendMessage } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectSession } from "../../adapters/store/selectors"

/**
 * The decision an agent in this session asked the person, while the session's
 * list row says it is still asked. A tapped choice is sent as the person's
 * own message, the same as typing it; the gateway then says nothing is asked
 * and the card leaves. A choice that was not sent stays in the outbox to send
 * again, and the card offers every choice again.
 */
export function OwnerAsk({ sessionId }: { sessionId: string }) {
  const dispatch = useWorkspaceDispatch()
  const ask = useWorkspaceSelector((state) => selectSession(state, sessionId)?.ask)
  const [sending, setSending] = useState<{ askId: string; label: string } | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  // A new ask starts with nothing on its way.
  const askId = ask?.askId
  useEffect(() => {
    setSending(null)
    setFailure(null)
  }, [askId])
  if (!ask) return null
  return (
    <OwnerAskCard
      ask={ask}
      sending={sending?.askId === ask.askId ? sending.label : null}
      failure={failure}
      onChoose={(label) => {
        setSending({ askId: ask.askId, label })
        setFailure(null)
        void dispatch(sendMessage({ sessionId, text: label, initiator: "person" })).then(
          (outcome) => {
            if (outcome === "sent") return
            setSending(null)
            setFailure(
              outcome === "not-asked"
                ? "This choice could not be sent here. Write your answer instead."
                : "This choice was not sent. Pick it again, or write your answer.",
            )
          },
        )
      }}
    />
  )
}
