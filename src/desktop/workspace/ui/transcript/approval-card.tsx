import { memo } from "react"
import { DesktopIcon } from "../../../ui/icons"
import { approve, deny } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectAnswer } from "../../adapters/store/selectors"
import { answering } from "../../application/workspace-state"
import { agentName, agentOf, type ModelRef } from "../../model/organisation"
import type { Approval } from "../../model/transcript"
import "./approval-card.css"

/**
 * The one warm thing on the page: a command the agent waits to run. Deny
 * stands on its own; the two ways to allow stay together, even wrapped.
 * While an answer is on its way the buttons rest; a failed answer says why.
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
      <pre className="workspace-approval-command">
        <span aria-hidden="true">$ </span>
        {approval.command}
      </pre>
      <p className="workspace-approval-reason">{approval.reason}</p>
      {answer?.failure ? (
        <p className="workspace-approval-failure" role="status">
          {answer.failure}
        </p>
      ) : null}
      <div className="workspace-approval-actions">
        <button
          type="button"
          className="workspace-button"
          disabled={waiting}
          onClick={() =>
            void dispatch(
              deny({ sessionId, approvalId: approval.id, initiator: "person" }),
            )
          }
        >
          Deny
        </button>
        <div className="workspace-approval-allow">
          <button
            type="button"
            className="workspace-button"
            disabled={waiting}
            onClick={() =>
              void dispatch(
                approve({
                  sessionId,
                  approvalId: approval.id,
                  scope: "always",
                  initiator: "person",
                }),
              )
            }
          >
            Always Allow
          </button>
          <button
            type="button"
            className="workspace-button"
            data-primary
            disabled={waiting}
            onClick={() =>
              void dispatch(
                approve({
                  sessionId,
                  approvalId: approval.id,
                  scope: "once",
                  initiator: "person",
                }),
              )
            }
          >
            Allow Once
          </button>
        </div>
      </div>
    </div>
  )
})
