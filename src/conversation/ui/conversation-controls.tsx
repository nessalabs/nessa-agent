import { deadlinePendingLine, permissionTargets } from "@nessa/client"
import { historyExtent, historyNotices, type Conversation } from "../model"
import { controlConversation } from "../adapters/store/slice"
import { useConversationDispatch } from "../adapters/store/hooks"
import { useNow } from "../../clock"

import { ShieldCheck } from "lucide-react"
import {
  ToolApproval,
  ToolApprovalHeader,
  ToolApprovalIcon,
  ToolApprovalHeading,
  ToolApprovalTitle,
  ToolApprovalDescription,
  ToolApprovalCommand,
  ToolApprovalActions,
  ToolApprovalAction,
} from "@nessa-ui/react/tool-approval"

/**
 * Render exact offered choices. A review's input is shown whole before any
 * choice is offered: one whose input has not been read whole yet shows its
 * start and offers nothing but dismissing it.
 */
export function ConversationControls({
  conversation,
  gatewayAvailable,
}: {
  conversation: Conversation
  gatewayAvailable: boolean
}) {
  const dispatch = useConversationDispatch()
  const remote = conversation.remote
  const targets =
    remote && conversation.serverConversationId
      ? permissionTargets({
          conversationId: conversation.serverConversationId,
          permissions: remote.permissions,
        })
      : []
  const now = useNow(1000)
  return (
    <div className="flex flex-col gap-3 text-sm">
      {historyNotices(historyExtent(remote)).map((notice) => (
        <p key={notice.kind} role="status">
          {notice.text}
        </p>
      ))}
      {remote?.permissions.map((permission, index) => (
        <ToolApproval
          key={JSON.stringify([permission.executionId, permission.permissionId])}
          variant="floating"
          aria-label={`Approve ${permission.title}`}
          aria-busy={conversation.controlPending}
          className="self-center"
        >
          <ToolApprovalHeader>
            <ToolApprovalIcon>
              <ShieldCheck aria-hidden="true" />
            </ToolApprovalIcon>
            <ToolApprovalHeading>
              <ToolApprovalTitle>{permission.title}</ToolApprovalTitle>
              <ToolApprovalDescription>
                {permission.deadline
                  ? `${permission.toolName}. ${deadlinePendingLine(permission.deadline, now)}`
                  : permission.toolName}
              </ToolApprovalDescription>
            </ToolApprovalHeading>
          </ToolApprovalHeader>
          <ToolApprovalCommand json={permission.argumentsJson} />
          {permission.argumentsComplete ? null : (
            // Only the start is here. No choice is offered on what has not
            // been seen whole; the rest is being read (`review-inputs.ts`).
            <p role="status">Loading the whole input before you can answer…</p>
          )}
          <ToolApprovalActions>
            {permission.options.map((option) => (
              <ToolApprovalAction
                key={option.id}
                disabled={
                  !gatewayAvailable ||
                  conversation.controlPending ||
                  !remote.capabilities.permissions ||
                  !permission.argumentsComplete
                }
                onClick={() => {
                  const target = targets[index]
                  if (!target) return
                  void dispatch(
                    controlConversation({
                      id: conversation.id,
                      control: {
                        kind: "answer",
                        target,
                        optionId: option.id,
                      },
                    }),
                  )
                }}
              >
                {option.label}
              </ToolApprovalAction>
            ))}
            <ToolApprovalAction
              variant="ghost"
              disabled={!gatewayAvailable || conversation.controlPending}
              onClick={() => {
                const target = targets[index]
                if (!target) return
                void dispatch(
                  controlConversation({
                    id: conversation.id,
                    control: {
                      kind: "cancel",
                      target,
                    },
                  }),
                )
              }}
            >
              Dismiss request
            </ToolApprovalAction>
          </ToolApprovalActions>
        </ToolApproval>
      ))}
      {conversation.cancellationStatus ? (
        <p role="status">
          {conversation.cancellationStatus === "cancelling" ? "Cancelling…" : "Cancelled"}
        </p>
      ) : null}
    </div>
  )
}
