import { memo } from "react"
import { DesktopIcon } from "../../../ui/icons"
import { approve, deny } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectAnswer } from "../../adapters/store/selectors"
import { answering } from "../../application/workspace-state"
import { agentName, agentOf, type ModelRef } from "../../model/workspace-index"
import type { Approval } from "../../model/transcript"
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
      {answer?.failure ? (
        <p className="workspace-approval-failure" role="status">
          {failureCopy(answer.failure)}
        </p>
      ) : null}
      <ApprovalActions
        options={approval.options}
        disabled={waiting}
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
