import { useEffect, useState } from "react"
import {
  AccessRequestCard,
  isAccessRequest,
  OwnerAskCard,
} from "../../../../conversation"
import { chooseAccess, sendMessage } from "../../adapters/store/commands"
import { modelChanging } from "../../adapters/store/model-commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectAtWork, selectSession } from "../../adapters/store/selectors"
import { entry } from "../../application/workspace-state"
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
 * composer's shield, and sends Approve as their answer only once the
 * source's conversation says the session runs at that access, the end state
 * and not the command's reply: a choice held for the end of a turn is not a
 * change yet. The card stays, listing what was allowed, after the list lets
 * the ask go. While the agent is at work Approve waits, since access changes
 * only between turns.
 */
export function OwnerAsk({ sessionId }: { sessionId: string }) {
  const dispatch = useWorkspaceDispatch()
  const ask = useWorkspaceSelector((state) => selectSession(state, sessionId)?.ask)
  const working = useWorkspaceSelector((state) => selectAtWork(state, sessionId))
  // A message waits while the session's access is still changing, as the
  // composer's does; Approve's answer is sent once it has settled at the
  // access asked for.
  const settling = useWorkspaceSelector((state) =>
    modelChanging(state.workspace, sessionId),
  )
  const runsAt = useWorkspaceSelector((state) => {
    const access = entry(state.workspace.transcripts, sessionId)?.access
    return access && access.changing === undefined ? access.mode : undefined
  })
  const refused = useWorkspaceSelector(
    (state) => entry(state.workspace.accessChoices, sessionId)?.state === "refused",
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
    setDue(null)
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
  const raised = due && isAccessRequest(due.ask) ? due.ask.access.mode : undefined
  useEffect(() => {
    if (!due) return
    if (refused) {
      // The change held for the turn's end was refused when it was asked.
      setDue(null)
      setSending(null)
      setFailure("Access didn’t change. Press Approve again, or write your answer.")
      return
    }
    if (settling || working || raised === undefined || runsAt !== raised) return
    setDue(null)
    void answer(
      due.label,
      "Access changed, but your answer was not sent. Write it to the agent.",
    ).then((sent) => {
      if (sent) setApproved(due.ask)
    })
    // `answer` is this render's; only the due answer and the access start it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [due, refused, settling, working, raised, runsAt])
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
            // waits until the session runs at it.
            if (outcome !== "taken" && outcome !== "pending") {
              setSending(null)
              setFailure(
                outcome === "changing"
                  ? "Access is still changing. Press Approve once it settles."
                  : "Access didn’t change. Press Approve again, or write your answer.",
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
          void answer(
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
        void answer(
          label,
          "This choice was not sent. Pick it again, or write your answer.",
        )
      }}
    />
  )
}
