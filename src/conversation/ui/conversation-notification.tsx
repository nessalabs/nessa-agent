import { AgentNotification } from "@nessa-ui/react/agent-notification"
import type { Conversation } from "../model"
import {
  controlConversation,
  refreshConversation,
  sendDraft,
} from "../adapters/store/slice"
import { useConversationDispatch } from "../adapters/store/hooks"
import { conversationNotice } from "./notification"
import type { SessionPhase } from "../../session"

/** Transport recovery and explicit submission retry remain separate commands. */
export function ConversationNotification({
  conversation,
  connection,
  gatewayAvailable,
}: {
  conversation: Conversation
  connection: { phase: SessionPhase; detail: string; retry: () => void }
  gatewayAvailable: boolean
}) {
  const dispatch = useConversationDispatch()
  if (connection.phase === "reconnecting")
    return (
      <AgentNotification
        className="mb-2"
        state="reconnecting"
        description="Restoring the gateway connection. Messages are not resent."
      />
    )
  if (connection.phase === "error")
    return (
      <AgentNotification
        className="mb-2"
        state="disconnected"
        description={connection.detail}
        onRetry={connection.retry}
      />
    )
  const notice = conversationNotice(conversation)
  if (!notice) return null
  return (
    <AgentNotification
      className="mb-2"
      state="disconnected"
      title={notice.title}
      description={notice.description}
      retryLabel={
        notice.retry?.kind === "refresh" ? "Refresh conversation" : "Retry message"
      }
      // Unreachable, not absent. Whether the gateway is up and whether a
      // control is already in flight both change while the notice is on
      // screen, and withdrawing `onRetry` for either would unmount the control
      // — out from under anybody whose focus is on it, which then lands on the
      // notice's own text and paints a focus ring around it. The notice keeps
      // offering the action and lets the component disable it in place.
      retryPending={!gatewayAvailable || Boolean(conversation.controlPending)}
      onRetry={
        notice.retry
          ? () => {
              if (notice.retry?.kind === "submission") {
                void dispatch(
                  controlConversation({
                    id: conversation.id,
                    control: { kind: "retry", executionId: notice.retry.executionId },
                  }),
                )
              } else if (notice.retry?.kind === "draft") {
                void dispatch(
                  sendDraft({ id: conversation.id, content: conversation.draft }),
                )
              } else {
                void dispatch(refreshConversation(conversation.id))
              }
            }
          : undefined
      }
    />
  )
}
