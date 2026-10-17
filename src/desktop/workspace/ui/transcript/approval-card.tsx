import { deadlinePendingLine } from "@nessa/client"
import { memo } from "react"
import { useNow } from "../../../../clock"
import { DesktopIcon } from "../../../ui/icons"
import { approve, deny, retryReview } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectAnswer } from "../../adapters/store/selectors"
import { answering } from "../../application/workspace-state"
import { agentName, agentOf, type ModelRef } from "../../model/workspace-index"
import { shownWhole, type Approval } from "../../model/transcript"
import { failureCopy } from "../failure-copy"
import {
  ApprovalActions,
  ApprovalCommand,
  approvalHead,
  approvalReason,
} from "./approval-request"
import { Saying } from "./said"

/**
 * The one warm thing on the page: a command the agent — or an MCP App, which
 * the head then names (`approvalHead`) — waits to run, and its
 * answers, arranged for the card's width (`approval-request.tsx`). While an
 * answer is on its way the buttons rest; a failed answer says why.
 */
export const ApprovalCard = memo(function ApprovalCard({
  sessionId,
  approval,
  model,
}: {
  sessionId: string
  approval: Approval
  model: ModelRef
}) {
  const dispatch = useWorkspaceDispatch()
  const answer = useWorkspaceSelector((state) =>
    selectAnswer(state, sessionId, approval.id),
  )
  const waiting = answering(answer, approval.id)
  const now = useNow(1000)
  return (
    <div
      className="workspace-approval"
      role="group"
      aria-label="Approval needed"
      data-origin={approval.origin.kind}
      data-ask={approval.ask}
    >
      <div className="workspace-approval-head">
        <DesktopIcon name="needsYou" />
        <span className="workspace-approval-head-words">
          <Saying said={approvalHead(approval, agentName(agentOf(model)))} />
        </span>
      </div>
      <ApprovalCommand
        command={approval.command}
        name={approval.origin.kind === "app" ? approval.origin.tool : undefined}
      />
      <p className="workspace-approval-reason">
        <Saying said={approvalReason(approval)} />
      </p>
      {approval.deadline ? (
        <p
          className="workspace-approval-deadline"
          data-deadline={approval.deadline.effect}
        >
          <Saying said={[deadlinePendingLine(approval.deadline, now)]} />
        </p>
      ) : null}
      {approval.loading === "reading" ? (
        <p className="workspace-approval-loading" role="status">
          Loading this review before you can answer…
        </p>
      ) : approval.loading === "failed" ? (
        <p className="workspace-approval-loading" role="status">
          This review could not be loaded. Trying again…{" "}
          <button
            type="button"
            className="workspace-button workspace-approval-retry"
            onClick={() =>
              void dispatch(retryReview({ sessionId, approvalId: approval.id }))
            }
          >
            Try again
          </button>
        </p>
      ) : null}
      {answer?.failure ? (
        <p className="workspace-approval-failure" role="status">
          {failureCopy(answer.failure)}
        </p>
      ) : null}
      <ApprovalActions
        options={approval.options}
        // Not before all of what it allows is shown (`Approval.loading`).
        disabled={waiting || !shownWhole(approval)}
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
          )
        }
      />
    </div>
  )
})
