import { deadlinePendingLine, permissionTargets } from "@nessa/client"
import { historyExtent, historyNotices, type Conversation } from "../model"
import { controlConversation, refollowConversation } from "../adapters/store/slice"
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
 * Render exact offered choices. A review is shown, and its choices offered,
 * only once all it shows has been read whole from its live owner; until then
 * it says it is loading, or that it could not be loaded, and offers trying
 * again.
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
      {remote?.reviewsWaiting.map((waiting) => (
        // Listed, but what it shows is still being read: nothing is offered
        // on a review nobody has seen (`reviewDetails`).
        <ToolApproval
          key={waiting.review}
          variant="floating"
          aria-label={`Approve ${waiting.toolName}`}
          aria-busy={!waiting.failed}
          className="self-center"
        >
          <ToolApprovalHeader>
            <ToolApprovalIcon>
              <ShieldCheck aria-hidden="true" />
            </ToolApprovalIcon>
            <ToolApprovalHeading>
              <ToolApprovalTitle>{waiting.toolName}</ToolApprovalTitle>
              <ToolApprovalDescription role="status">
                {waiting.failed
                  ? "This review could not be loaded. Trying again…"
                  : "Loading this review before you can answer…"}
              </ToolApprovalDescription>
            </ToolApprovalHeading>
          </ToolApprovalHeader>
          {waiting.failed ? (
            <ToolApprovalActions>
              <ToolApprovalAction
                variant="ghost"
                disabled={!gatewayAvailable}
                onClick={() => void dispatch(refollowConversation(conversation.id))}
              >
                Try again
              </ToolApprovalAction>
            </ToolApprovalActions>
          ) : null}
        </ToolApproval>
      ))}
      {remote?.permissions.map((permission, index) => (
        <ToolApproval
          // The review's key beside its ids: one opened again under the same
          // ids is a new card.
          key={JSON.stringify([
            permission.review,
            permission.executionId,
            permission.permissionId,
          ])}
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
          <ToolApprovalActions>
            {permission.options.map((option) => (
              <ToolApprovalAction
                key={option.id}
                disabled={
                  !gatewayAvailable ||
                  conversation.controlPending ||
                  !remote.capabilities.permissions
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
