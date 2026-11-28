import { SelectedAgentSetup } from "../../../onboarding/ui/selected-agent-setup"
import { agentForProvider } from "../../model/composer-options"
import { SendFailure } from "../../../conversation"
import { discardUnsent, resendMessage } from "../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../adapters/store/hooks"
import { useRememberedValue } from "../adapters/store/remembered-session"
import { selectFailedMessage } from "../adapters/store/selectors"
import { failureCopy } from "./failure-copy"
import { agentName, agentOf } from "../model/workspace-index"
import { BoatSignIn, useBoatConnection } from "../../settings/ui/boat-row"

/**
 * What a chat's dock offers for its refused message: only a way to fix
 * where it was refused (the agent's setup, Boat's connection). The message
 * itself says it did not go, and offers Send again and Dismiss, right under
 * it in the transcript (`UnsentRow`).
 */
export function WorkspaceSendRecovery({ sessionId }: { sessionId: string }) {
  const live = useWorkspaceSelector((state) => selectFailedMessage(state, sessionId))
  const message = useRememberedValue(sessionId, live)
  if (!message || message.delivery?.state !== "failed") return null
  const { diagnostic, reason, model } = message.delivery
  const agent = model ? agentForProvider(model.provider) : undefined
  return (
    <>
      {reason === "agent-not-configured" && agent ? (
        <SelectedAgentSetup agent={agent} recovery />
      ) : null}
      {diagnostic?.code === "environment_not_configured" ? <BoatRecovery /> : null}
    </>
  )
}

/**
 * The overview's reply pill, which shows no transcript, acts on the refused
 * message from a card above it.
 */
export function WorkspaceSendFailure({ sessionId }: { sessionId: string }) {
  const dispatch = useWorkspaceDispatch()
  const live = useWorkspaceSelector((state) => selectFailedMessage(state, sessionId))
  const message = useRememberedValue(sessionId, live)
  if (!message || message.delivery?.state !== "failed") return null
  const { diagnostic, reason, model } = message.delivery
  const agent = model ? agentForProvider(model.provider) : undefined
  return (
    <>
      <SendFailure
        details={diagnostic?.message ?? failureCopy(reason)}
        code={diagnostic?.code}
        summary={
          diagnostic?.code === "agent_sign_in_required" && model
            ? `${agentName(agentOf(model))} isn’t signed in where this chat runs.`
            : failureCopy(reason)
        }
        onRetry={() =>
          void dispatch(
            resendMessage({ sessionId, messageId: message.id, initiator: "person" }),
          )
        }
        onDiscard={() => dispatch(discardUnsent({ sessionId, messageId: message.id }))}
      />
      {reason === "agent-not-configured" && agent ? (
        <SelectedAgentSetup agent={agent} recovery />
      ) : null}
      {diagnostic?.code === "environment_not_configured" ? <BoatRecovery /> : null}
    </>
  )
}

/**
 * A message refused because where its chat runs is not set up: when Boat is
 * not connected, that is the likely place, so connecting it is offered here.
 */
function BoatRecovery() {
  const boat = useBoatConnection()
  if (boat.unread || boat.status === undefined || boat.status.state === "connected")
    return null
  return (
    <div className="settings-boat" data-boat-connect>
      <p role="status">Chats on Boat need Boat connected on this Nessa.</p>
      <BoatSignIn connection={boat} />
      {boat.error && <p role="alert">{boat.error}</p>}
    </div>
  )
}
