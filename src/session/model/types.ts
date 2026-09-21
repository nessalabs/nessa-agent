import type { HealthResult, ProductSessionReady } from "@nessa/client"

export type SessionPhase = "idle" | "connecting" | "reconnecting" | "ready" | "error"

export type SessionState = {
  /** Explicit reconnect command; not a transport state. */
  retryRequest: number
  phase: SessionPhase
  /** Human-readable detail for the empty state. */
  detail: string
  hello: ProductSessionReady | null
  health: HealthResult | null
}

type GatewayReadySession = SessionState & {
  phase: "ready"
  hello: ProductSessionReady
}

/** Gateway effects are available only while the authenticated session is ready. */
export function canUseGateway(session: SessionState): session is GatewayReadySession {
  return session.phase === "ready" && session.hello !== null
}

export function initialSessionState(): SessionState {
  return {
    retryRequest: 0,
    phase: "idle",
    detail: "Starting…",
    hello: null,
    health: null,
  }
}

export function statusLabel(state: SessionState): string {
  switch (state.phase) {
    case "idle":
    case "connecting":
      return "Connecting to the local server…"
    case "reconnecting":
      return "Reconnecting to the local server…"
    case "ready":
      return "Connected"
    case "error":
      return state.detail || "Could not reach the server. Run just server."
  }
}
