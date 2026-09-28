import { NessaRpcError } from "@nessa/client"
import type { AgentsApi } from "@nessa/client"
import type {
  AgentInstallations,
  InstallationOutcome,
} from "../application/agent-installations"

/** Uses the composition-owned authenticated session; no URL or path comes from UI. */
export function gatewayAgentInstallations(
  agents: () => AgentsApi | undefined,
  requestId: () => string,
  subscribe: (listener: () => void) => () => void,
): AgentInstallations {
  return {
    subscribe,
    async offers() {
      const api = agents()
      if (!api) throw new Error("Gateway is not connected")
      return (await api.installOptions()).agents
    },
    async install(agent): Promise<InstallationOutcome> {
      const api = agents()
      if (!api) return { status: "failed", reason: "unavailable" }
      try {
        const result = await api.install(agent, requestId())
        if (result.agent !== agent) return { status: "failed", reason: "not-confirmed" }
        return { status: "installed", cleanupPending: result.cleanupPending }
      } catch (error) {
        if (error instanceof NessaRpcError) {
          switch (error.code) {
            case "agent_install_storage_failed":
              return { status: "failed", reason: "storage" }
            case "agent_download_failed":
              return { status: "failed", reason: "download" }
            case "agent_install_verification_failed":
              return { status: "failed", reason: "verification" }
            case "agent_install_busy":
              return { status: "failed", reason: "busy" }
            case "agent_install_unsupported":
            case "agent_installer_unavailable":
              return { status: "failed", reason: "unavailable" }
          }
        }
        return { status: "failed", reason: "not-confirmed" }
      }
    },
  }
}
