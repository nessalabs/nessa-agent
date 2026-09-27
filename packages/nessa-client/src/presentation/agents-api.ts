import type { AgentsListResult } from "../generated/product.js"
import { ProductMethod } from "../generated/product.js"
import type { RpcRequester } from "../application/session-port.js"
import { agentsList } from "../protocol/agents-validate.js"

/** Configured agent and model choices published by the authenticated gateway. */
export type AgentsApi = {
  /** Read the gateway's current choices. Each model carries its verified presets. */
  list(): Promise<AgentsListResult>
}

export function createAgentsApi(session: RpcRequester): AgentsApi {
  return {
    list: async () => agentsList(await session.request(ProductMethod.AgentsList, {})),
  }
}
