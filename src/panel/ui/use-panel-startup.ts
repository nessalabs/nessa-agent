import * as React from "react"
import type { SessionPhase } from "../../session"
import { useGatewayStartup } from "../../startup"

/**
 * The host's startup projection, for the panel (ADR 221). A session that gave
 * up while the gateway was starting is retried once, when the host says it has
 * become ready, rather than waiting for somebody to press Retry. Only the
 * change to ready does this, so a session that fails again against a ready
 * gateway keeps its own notice and Retry.
 */
export function usePanelStartup(session: { phase: SessionPhase; retry: () => void }) {
  const startup = useGatewayStartup()
  const ready = startup.status?.state === "ready"
  const wasReady = React.useRef(ready)
  const retrySession = React.useRef(session.retry)
  retrySession.current = session.retry
  const sessionFailed = session.phase === "error"
  React.useEffect(() => {
    const becameReady = ready && !wasReady.current
    wasReady.current = ready
    if (becameReady && sessionFailed) retrySession.current()
  }, [ready, sessionFailed])
  return startup
}
