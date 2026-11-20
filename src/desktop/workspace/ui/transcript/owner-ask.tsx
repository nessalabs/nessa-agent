import { useEffect, useState } from "react"
import { AccessRequestCard, isAccessRequest, OwnerAskCard } from "../../../../conversation"
import { chooseAccess, sendMessage } from "../../adapters/store/commands"
import { modelChanging } from "../../adapters/store/model-commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectAtWork, selectSession } from "../../adapters/store/selectors"
import type { OwnerAsk as Ask } from "../../model/workspace-index"

/**
 * The decision an agent in this session asked the person, while the session's
 * list row says it is still asked. A tapped choice is sent as the person's
 * own message, the same as typing it; the gateway then says nothing is asked
 * and the card leaves. A choice that was not sent stays in the outbox to send
 * again, and the card offers every choice again.
 *
 * An agent's request for more access is the red card. Approve raises the
 * session's access by the person's own access command, the same as the
 * composer's shield, and only then sends Approve as their answer; the card
 * stays, listing what was allowed, after the list lets the ask go. While the
 * agent is at work Approve waits, since access changes only between turns.
 */
export function OwnerAsk({ sessionId }: { sessionId: string }) {
  const dispatch = useWorkspaceDispatch()
  const ask = useWorkspaceSelector((state) => selectSession(state, sessionId)?.ask)
  const working = useWorkspaceSelector((state) => selectAtWork(state, sessionId))
  // A message waits while the session's access is still changing, as the
  // composer's does; Approve's answer is sent once it has settled.
  const settling = useWorkspaceSelector((state) =>
    modelChanging(state.workspace, sessionId),
  )
  const [sending, setSending] = useState<{ askId: string; label: string } | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  // The access request just approved, kept to say what was allowed.
  const [approved, setApproved] = useState<Ask | null>(null)
  // Approve's answer, due once the access it raised has settled.
  const [due, setDue] = useState<{ ask: Ask; label: string } | null>(null)
  // A new ask starts with nothing on its way.
  const askId = ask?.askId
  useEffect(() => {
    setSending(null)
    setFailure(null)
  }, [askId])
  const answer = (label: string, notSent: string) =>
    dispatch(sendMessage({ sessionId, text: label, initiator: "person" })).then(
      (outcome) => {
        if (outcome === "sent") return true
        setSending(null)
        setFailure(
          outcome === "not-asked"
            ? "This choice could not be sent here. Write your answer instead."
            : notSent,
        )
        return false
      },
    )
  useEffect(() => {
    if (!due || settling) return
    setDue(null)
    void answer(
      due.label,
      "Access changed, but your answer was not sent. Write it to the agent.",
    ).then((sent) => {
      if (sent) setApproved(due.ask)
    })
    // `answer` is this render's; only the due answer and the settling start it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [due, settling])
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
          void dispatch(
            chooseAccess({ sessionId, mode: ask.access.mode, initiator: "person" }),
          ).then((outcome) => {
            // Taken now, or held for the end of a turn that began first:
            // either way the person's access command stands, and the answer
            // goes after it.
            if (outcome !== "taken" && outcome !== "pending") {
              setSending(null)
              setFailure(
                "Access didn’t change. Press Approve again, or write your answer.",
              )
              return
            }
            setDue({ ask, label: approve.label })
          })
        }}
        onDecline={() => {
          if (!decline) return
          setSending({ askId: ask.askId, label: decline.label })
          setFailure(null)
          void answer(decline.label, "Not now was not sent. Press it again, or write your answer.")
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
        void answer(label, "This choice was not sent. Pick it again, or write your answer.")
      }}
    />
  )
}
