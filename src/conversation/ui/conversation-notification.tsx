import { SelectedAgentSetup } from "../../onboarding/ui/selected-agent-setup"
import { SendFailure } from "./send-failure"
import { executionTargets } from "@nessa/client"
import { AgentNotification } from "@nessa-ui/react/agent-notification"
import type { Conversation } from "../model"
import {
  controlConversation,
  followConversation,
  retryFailedTurn,
  failedTurnDiscarded,
  canSendAgain,
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
        // This notice is the only account of the failure: the label under the
        // avatar names the state and leaves the reason here. An `Error` whose
        // message is empty reaches `detail` as "", which would leave a Retry
        // button with nothing said about what it retries.
        description={connection.detail || "The local server could not be reached."}
        onRetry={connection.retry}
      />
    )
  const targets = conversation.serverConversationId
    ? executionTargets({
        conversationId: conversation.serverConversationId,
        messages: conversation.turns.flatMap((turn) =>
          turn.executionId ? [{ executionId: turn.executionId }] : [],
        ),
        pending: [],
      })
    : []
  const notice = conversationNotice(conversation)
  if (!notice)
    return conversation.turns.length === 0 && conversation.selection?.agent ? (
      <SelectedAgentSetup agent={conversation.selection.agent} />
    ) : null
  if (notice.retry?.kind === "draft") {
    const turnId = notice.retry.turnId
    const failed = conversation.turns.find((turn) => turn.id === turnId)
    return (
      <>
        <SendFailure
          details={
            failed?.from === "user"
              ? (failed.diagnostic?.message ?? failed.error ?? notice.description)
              : notice.description
          }
          code={failed?.from === "user" ? failed.diagnostic?.code : undefined}
          pending={
            !gatewayAvailable ||
            Boolean(conversation.controlPending) ||
            !canSendAgain(conversation, turnId)
          }
          onRetry={() => void dispatch(retryFailedTurn({ id: conversation.id, turnId }))}
          onDiscard={() => dispatch(failedTurnDiscarded({ id: conversation.id, turnId }))}
        />
        {conversation.failure === "agent-not-configured" &&
        failed?.from === "user" &&
        failed.diagnostic?.agent ? (
          <SelectedAgentSetup agent={failed.diagnostic.agent} recovery />
        ) : null}
      </>
    )
  }

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
                const target = targets.find(
                  (target) =>
                    target.executionId ===
                    (notice.retry?.kind === "submission"
                      ? notice.retry.executionId
                      : undefined),
                )
                if (!target) return
                void dispatch(
                  controlConversation({
                    id: conversation.id,
                    control: { kind: "retry", target },
                  }),
                )
              } else {
                void dispatch(followConversation(conversation.id))
              }
            }
          : undefined
      }
    />
  )
}
