import { useEffect, useState } from "react"
import { AccessRequestHost, OwnerAskCard } from "../../../../conversation"
import { chooseAccess, sendMessage } from "../../adapters/store/commands"
import { modelChanging } from "../../adapters/store/model-commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectAtWork, selectSession } from "../../adapters/store/selectors"
import { entry } from "../../application/workspace-state"

/**
 * The decision an agent in this session asked the person, while the session's
 * list row says it is still asked. A tapped choice is sent as the person's
 * own message, the same as typing it; the gateway then says nothing is asked
 * and the card leaves. A choice that was not sent stays in the outbox to send
 * again, and the card offers every choice again.
 *
 * An agent's request for more access is the red card, whose round is
 * `AccessRequestHost`'s, the same as the panel's: Approve raises the
 * session's access by the person's own access command, the same as the
 * composer's shield, and answers once the source's conversation says the
 * session runs at it, every fact here as its owner states it.
 */
export function OwnerAsk({ sessionId }: { sessionId: string }) {
  const dispatch = useWorkspaceDispatch()
  const ask = useWorkspaceSelector((state) => selectSession(state, sessionId)?.ask)
  const working = useWorkspaceSelector((state) => selectAtWork(state, sessionId))
  // A message waits while the session's access or model is still changing,
  // as the composer's does: the chat runs at nothing settled until then.
  const runsAt = useWorkspaceSelector((state) => {
    if (modelChanging(state.workspace, sessionId)) return undefined
    const access = entry(state.workspace.transcripts, sessionId)?.access
    return access && access.changing === undefined ? access.mode : undefined
  })
  // Where a change on its way is headed: the source's own, or the person's
  // choice held for a turn's end or asked of the source. A taken choice is
  // not one: the source has it, and its conversation says where it landed.
  const underway = useWorkspaceSelector((state) => {
    const choice = entry(state.workspace.accessChoices, sessionId)
    if (choice?.state === "pending" || choice?.state === "changing") return choice.mode
    return entry(state.workspace.transcripts, sessionId)?.access?.changing
  })
  const revision = useWorkspaceSelector(
    (state) => entry(state.workspace.transcripts, sessionId)?.revision,
  )
  const [sending, setSending] = useState<{ askId: string; label: string } | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  // A new ask starts with nothing on its way.
  const askId = ask?.askId
  useEffect(() => {
    setSending(null)
    setFailure(null)
  }, [askId])
  const answer = (text: string) =>
    dispatch(sendMessage({ sessionId, text, initiator: "person" })).then(
      (outcome) => outcome === "sent",
    )
  return (
    <>
      <AccessRequestHost
        ask={ask}
        runsAt={runsAt}
        underway={underway}
        working={working}
        revision={revision}
        raise={(mode) =>
          dispatch(chooseAccess({ sessionId, mode, initiator: "person" })).then(
            (outcome) =>
              outcome === "taken" || outcome === "pending" || outcome === "changing"
                ? outcome
                : "refused",
          )
        }
        answer={answer}
      />
      {ask && !ask.access ? (
        <OwnerAskCard
          ask={ask}
          sending={sending?.askId === ask.askId ? sending.label : null}
          failure={failure}
          onChoose={(label) => {
            setSending({ askId: ask.askId, label })
            setFailure(null)
            void dispatch(
              sendMessage({ sessionId, text: label, initiator: "person" }),
            ).then((outcome) => {
              if (outcome === "sent") return
              setSending(null)
              setFailure(
                outcome === "not-asked"
                  ? "This choice could not be sent here. Write your answer instead."
                  : "This choice was not sent. Pick it again, or write your answer.",
              )
            })
          }}
        />
      ) : null}
    </>
  )
}
