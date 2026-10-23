/**
 * What a waiting row in the session list asks, answered from the list: the
 * transcript card's own parts, compact (`transcript/approval-request.tsx`).
 * Under the row's agent and title, who asks and the exact command — clipped
 * to a few lines with a fade, whole in the conversation — then its answers,
 * in the same place on every card whatever the command's length. A question
 * shows the question card. The first request is shown; more wait as "+N
 * more", in the conversation. While answered, the card leaves on the
 * motion's curve; a refused answer says why in place.
 *
 * The list reads and keeps the conversations it asks about
 * (`listAskIds`, `workspace-state.ts`), so what is shown here is what an
 * answer answers (`onScreen`).
 */
import { memo, useLayoutEffect, useRef, useState } from "react"
import { approve, deny, openSession } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectAnswer,
  selectSession,
  selectTranscript,
} from "../../adapters/store/selectors"
import { answering } from "../../application/workspace-state"
import { shownWhole } from "../../model/transcript"
import { agentName, agentOf } from "../../model/workspace-index"
import { failureCopy } from "../failure-copy"
import {
  ApprovalActions,
  ApprovalCommand,
  approvalHead,
} from "../transcript/approval-request"
import { approvalGist } from "../transcript/approval-card"
import { QuestionCard } from "../transcript/question-card"
import { Saying, shownName } from "../transcript/said"
import "../transcript/transcript.css"
import "./list-request.css"

export const ListRequest = memo(function ListRequest({
  sessionId,
}: {
  sessionId: string
}) {
  const dispatch = useWorkspaceDispatch()
  const session = useWorkspaceSelector((state) => selectSession(state, sessionId))
  const transcript = useWorkspaceSelector((state) => selectTranscript(state, sessionId))
  const approvals = transcript?.approvals ?? []
  const questions = transcript?.questions ?? []
  const approval = approvals[0] ?? null
  const answer = useWorkspaceSelector((state) =>
    approval ? selectAnswer(state, sessionId, approval.id) : undefined,
  )
  // An answer the window did not send: the request is no longer asked from here.
  const [unsent, setUnsent] = useState(false)
  // A command taller than the card shows, which no one should allow unseen.
  const command = useRef<HTMLDivElement>(null)
  const [clipped, setClipped] = useState(false)
  useLayoutEffect(() => {
    const box = command.current
    const pre = box?.querySelector("pre")
    if (!box || !pre) return setClipped(false)
    setClipped(pre.scrollHeight > box.clientHeight + 1)
  }, [approval?.id, approval?.command])
  if (!session) return null
  const asks = approvals.length + questions.length
  const more =
    asks > 1 ? <span className="workspace-request-more">+{asks - 1} more</span> : null
  if (!approval) {
    const question = questions[0]
    if (!question) return null
    return (
      <div
        className="workspace-request"
        role="group"
        aria-label={`Question from ${session.title}`}
        data-request="question"
      >
        <QuestionCard sessionId={sessionId} question={question} />
        {more}
      </div>
    )
  }
  const sent = answering(answer, approval.id)
  // The transcript card's own reading of the command (`approvalGist`).
  const gist = approvalGist(approval)
  const answerable = shownWhole(approval) && !sent
  return (
    <div
      className="workspace-request"
      role="group"
      aria-label={`Request from ${session.title}`}
      data-request="approval"
      data-answering={sent || undefined}
    >
      <p className="workspace-request-head">
        <Saying said={approvalHead(approval, agentName(agentOf(session.model)))} />
      </p>
      <div
        className="workspace-request-command"
        ref={command}
        data-clipped={clipped || undefined}
      >
        {approval.loading ? (
          <p className="workspace-request-loading" role="status">
            {approval.loading === "reading"
              ? "Loading this review…"
              : "This review could not be loaded."}
          </p>
        ) : (
          <ApprovalCommand
            command={gist?.script ?? approval.command}
            name={approval.origin.kind === "app" ? approval.origin.tool : undefined}
          />
        )}
      </div>
      {gist?.tool ? (
        <p className="workspace-request-tool">
          Tool <bdi>{shownName(gist.tool)}</bdi>
        </p>
      ) : null}
      {answer?.failure ? (
        <p className="workspace-request-failure" role="status">
          {failureCopy(answer.failure)}
        </p>
      ) : unsent ? (
        <p className="workspace-request-failure" role="status">
          This request is no longer asked here. Open the chat to answer it.
        </p>
      ) : null}
      <div className="workspace-request-foot">
        {more}
        <ApprovalActions
          compact
          // Clipped, it is refused or read whole in the chat, never allowed unseen.
          options={
            clipped
              ? approval.options.filter((option) => option.choice === "deny")
              : approval.options
          }
          disabled={!answerable}
          onAnswer={(option) =>
            void dispatch(
              option.choice === "deny"
                ? deny({
                    sessionId,
                    approvalId: approval.id,
                    initiator: "person",
                    optionId: option.id,
                  })
                : approve({
                    sessionId,
                    approvalId: approval.id,
                    scope: option.choice,
                    initiator: "person",
                    optionId: option.id,
                  }),
            ).then((outcome) => setUnsent(outcome === "not-asked"))
          }
        />
        {clipped ? (
          <button
            type="button"
            className="workspace-button workspace-request-review"
            data-primary
            onClick={() => dispatch(openSession({ sessionId }))}
          >
            Review in Chat
          </button>
        ) : null}
      </div>
    </div>
  )
})
