import { memo } from "react"
import { DesktopIcon } from "../../../ui/icons"
import { approve, deny } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectAnswer } from "../../adapters/store/selectors"
import { answering } from "../../application/workspace-state"
import { agentName, agentOf, type ModelRef } from "../../model/workspace-index"
import type { Approval } from "../../model/transcript"
import { failureCopy } from "../failure-copy"
import { ApprovalActions, ApprovalCommand } from "./approval-request"

/**
 * The one warm thing on the page: a command the agent waits to run, and its
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
  return (
    <div className="workspace-approval" role="group" aria-label="Approval needed">
      <div className="workspace-approval-head">
        <DesktopIcon name="needsYou" />
        <span>{agentName(agentOf(model))} wants to run a command</span>
      </div>
      <ApprovalCommand command={approval.command} />
      <p className="workspace-approval-reason">{approval.reason}</p>
      {answer?.failure ? (
        <p className="workspace-approval-failure" role="status">
          {failureCopy(answer.failure)}
        </p>
      ) : null}
      <ApprovalActions
        disabled={waiting}
        onAnswer={(choice) =>
          void dispatch(
            choice === "deny"
              ? deny({ sessionId, approvalId: approval.id, initiator: "person" })
              : approve({
                  sessionId,
                  approvalId: approval.id,
                  scope: choice,
                  initiator: "person",
                }),
          )
        }
      />
    </div>
  )
})
