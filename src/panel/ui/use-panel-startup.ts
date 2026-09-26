import * as React from "react"
import type { SessionPhase } from "../../session"
import { useGatewayStartup } from "../../startup"

/**
 * The host's startup projection, for the panel (ADR 221). Each time the host
 * says the gateway has become ready, a session that has failed is retried once
 * — whether it failed before that moment or while still connecting after it —
 * rather than waiting for somebody to press Retry. A session that fails again
 * after that keeps its own notice and Retry.
 */
export function usePanelStartup(session: { phase: SessionPhase; retry: () => void }) {
  const startup = useGatewayStartup()
  const ready = startup.status?.state === "ready"
  const wasReady = React.useRef(ready)
  const retryOwed = React.useRef(false)
  const retrySession = React.useRef(session.retry)
  retrySession.current = session.retry
  const sessionFailed = session.phase === "error"
  React.useEffect(() => {
    if (ready && !wasReady.current) retryOwed.current = true
    if (!ready) retryOwed.current = false
    wasReady.current = ready
    if (retryOwed.current && sessionFailed) {
      retryOwed.current = false
      retrySession.current()
    }
  }, [ready, sessionFailed])
  return startup
}
