import type { Conversation } from "../model"
import { controlConversation } from "../adapters/store/slice"
import { useConversationDispatch } from "../adapters/store/hooks"

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

/** Render exact offered choices; permission input is never abbreviated before approval. */
export function ConversationControls({
  conversation,
  gatewayAvailable,
}: {
  conversation: Conversation
  gatewayAvailable: boolean
}) {
  const dispatch = useConversationDispatch()
  const remote = conversation.remote
  return (
    <div className="flex flex-col gap-3 text-sm">
      {remote?.truncated ? (
        <p role="status">Showing the most recent conversation history.</p>
      ) : null}
      {remote?.permissions.map((permission) => (
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
              <ToolApprovalDescription>{permission.toolName}</ToolApprovalDescription>
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
                  void dispatch(
                    controlConversation({
                      id: conversation.id,
                      control: {
                        kind: "answer",
                        executionId: permission.executionId,
                        permissionId: permission.permissionId,
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
                void dispatch(
                  controlConversation({
                    id: conversation.id,
                    control: {
                      kind: "cancel",
                      executionId: permission.executionId,
                      permissionId: permission.permissionId,
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
