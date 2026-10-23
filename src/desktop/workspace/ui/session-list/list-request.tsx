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
import { memo, useState } from "react"
import { approve, deny, openSession } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectAnswer,
  selectSession,
  selectTranscript,
} from "../../adapters/store/selectors"
import { answering } from "../../application/workspace-state"
import { shownWhole, type Approval } from "../../model/transcript"
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
import type { CommandGist } from "../transcript/approval-gist"

/** The most of a command, in lines and in characters, the list shows whole. */
export const listShownLines = 3
export const listShownChars = 180

/**
 * Whether the list may allow a request: only one it shows whole, decided
 * from the request alone, never from how it happened to lay out. An agent's
 * tool call read whole (`approvalGist`) with nothing in its input but the
 * command, and a command of at most `listShownLines` lines and
 * `listShownChars` characters; or an app's request of the same size. Any
 * other request is refused from the list, or read whole in the chat.
 */
export function allowableFromList(
  approval: Approval,
  gist: CommandGist | undefined,
): boolean {
  if (!shownWhole(approval)) return false
  if (gist && gist.fields.length > 0) return false
  const shown = (gist?.script ?? approval.command).replace(/\n$/, "")
  return shown.split("\n").length <= listShownLines && shown.length <= listShownChars
}

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
  // The request an answer was not sent for: it is no longer asked from here.
  const [unsentFor, setUnsentFor] = useState<string | null>(null)
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
  // Shown whole, it may be allowed here; else refused, or read in the chat.
  const whole = allowableFromList(approval, gist)
  const unsent = unsentFor === approval.id
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
      <div className="workspace-request-command" data-whole={whole || undefined}>
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
          // Not shown whole, it is refused or read whole in the chat, never allowed unseen.
          options={
            whole
              ? approval.options
              : approval.options.filter((option) => option.choice === "deny")
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
            ).then((outcome) =>
              setUnsentFor(outcome === "not-asked" ? approval.id : null),
            )
          }
        />
        {whole ? null : (
          <button
            type="button"
            className="workspace-button workspace-request-review"
            data-primary
            onClick={() => dispatch(openSession({ sessionId }))}
          >
            Review in Chat
          </button>
        )}
      </div>
    </div>
  )
})
