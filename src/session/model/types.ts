import type { HealthResult, HelloOk } from "@nessa/client"

export type SessionPhase = "idle" | "connecting" | "ready" | "error"

export type SessionState = {
  phase: SessionPhase
  /** Human-readable detail for the empty state. */
  detail: string
  hello: HelloOk | null
  health: HealthResult | null
}

export function initialSessionState(): SessionState {
  return {
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
    case "ready":
      return "Connected. Send a message — the server echoes it back."
    case "error":
      return state.detail || "Could not reach the server. Run just server."
  }
}
