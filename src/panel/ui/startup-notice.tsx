import { AgentNotification } from "@nessa-ui/react/agent-notification"
import {
  TRY_AGAIN_HINT,
  startupSentence,
  type GatewayStartupStatus,
} from "../../startup"

/**
 * The connection's own notice while Nessa is still starting, or could not
 * (ADR 221). It says what the host reports and nothing it has to guess: a step
 * while the host is working, one plain sentence and **Try again** once the
 * host has said it failed. `null` once there is nothing to say, when the
 * connection's own notice takes the slot back.
 */
export function startupNotice(
  status: GatewayStartupStatus | undefined,
  onRetry: () => void,
): React.ReactNode {
  // "Couldn't check" is about the page's link to the host, not about starting;
  // the session's own notice says what the connection is doing.
  if (!status || status.state === "unavailable") return null
  const sentence = startupSentence(status)
  if (!sentence) return null
  if (status.state === "starting")
    return <AgentNotification className="mb-2" state="reconnecting" description={sentence} />
  return (
    <AgentNotification
      className="mb-2"
      state="disconnected"
      title={sentence}
      description={TRY_AGAIN_HINT}
      retryLabel="Try again"
      onRetry={onRetry}
    />
  )
}
