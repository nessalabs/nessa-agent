import { SelectedAgentSetup } from "../../../onboarding"
import { agentForProvider } from "../../model/composer-options"
import { SendFailure } from "../../../conversation"
import { discardUnsent, resendMessage } from "../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../adapters/store/hooks"
import { selectFailedMessage } from "../adapters/store/selectors"
import { failureCopy } from "./failure-copy"

/** Both conversation and overview composers act on the same refused message. */
export function WorkspaceSendFailure({ sessionId }: { sessionId: string }) {
  const dispatch = useWorkspaceDispatch()
  const message = useWorkspaceSelector((state) => selectFailedMessage(state, sessionId))
  if (!message || message.delivery?.state !== "failed") return null
  const { diagnostic, reason, model } = message.delivery
  const agent = model ? agentForProvider(model.provider) : undefined
  return (
    <>
      <SendFailure
        details={diagnostic?.message ?? failureCopy(reason)}
        code={diagnostic?.code}
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
    </>
  )
}
