import type {
  AgentInstallOptionsResult,
  AgentInstallResult,
  AgentsListResult,
  InstallableAgent,
} from "../generated/product.js"
import { ProductMethod } from "../generated/product.js"
import type { RpcRequester } from "../application/session-port.js"
import {
  agentInstallOptions,
  agentInstallResult,
  agentsList,
} from "../protocol/agents-validate.js"

/** Configured agent and model choices published by the authenticated gateway. */
export type AgentsApi = {
  /** Read the gateway's current choices. Each model carries its verified presets. */
  list(): Promise<AgentsListResult>
  /** Read supported native downloads and current managed installation state. */
  installOptions(): Promise<AgentInstallOptionsResult>
  /** Download the pinned runtime on explicit request. A lost response does not
   * cancel installation; refresh installOptions before making a new attempt.
   * requestId identifies this attempt. The gateway retains audit attribution.
   */
  install(agent: InstallableAgent, requestId: string): Promise<AgentInstallResult>
}

export function createAgentsApi(session: RpcRequester): AgentsApi {
  return {
    installOptions: async () =>
      agentInstallOptions(await session.request(ProductMethod.AgentsInstallOptions, {})),
    install: async (agent, requestId) =>
      agentInstallResult(
        await session.request(
          ProductMethod.AgentsInstall,
          { agent, requestId },
          { atLeastMs: 46 * 60 * 1000 },
        ),
      ),
    list: async () => agentsList(await session.request(ProductMethod.AgentsList, {})),
  }
}
