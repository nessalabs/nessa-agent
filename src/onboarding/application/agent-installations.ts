import type { AgentId } from "../model/onboarding"

export type InstallableAgent = Extract<AgentId, "claude" | "codex" | "opencode">
export type InstallationOffer = {
  agent: InstallableAgent
  archiveBytes: number
  installed: boolean
}
export type InstallationOutcome =
  | { status: "installed"; cleanupPending: boolean }
  | {
      status: "failed"
      reason:
        | "download"
        | "refused"
        | "storage"
        | "verification"
        | "busy"
        | "unavailable"
        | "not-confirmed"
    }

/** The gateway owns publication; closing a view only abandons its wait. */
export interface AgentInstallations {
  /** Observe connection changes so first-run offers refresh after authentication. */
  subscribe(listener: () => void): () => void
  offers(): Promise<readonly InstallationOffer[]>
  install(agent: InstallableAgent): Promise<InstallationOutcome>
}
