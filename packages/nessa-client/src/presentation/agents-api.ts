import type {
  AgentInstallOptionsResult,
  AgentInstallResult,
  AgentsListResult,
  AgentsSetNativeHooksResult,
  InstallableAgent,
  NativeHooksAgent,
} from "../generated/product.js"
import { ProductMethod } from "../generated/product.js"
import type { RpcRequester } from "../application/session-port.js"
import {
  agentInstallOptions,
  agentInstallResult,
  agentInstallRequestId,
  agentsList,
  agentsSetNativeHooks,
  nativeHooksRequestId,
} from "../protocol/agents-validate.js"

/** Configured agent and model choices published by the authenticated gateway. */
export type AgentsApi = {
  /** Read the gateway's current choices. Each model carries its verified presets. */
  list(): Promise<AgentsListResult>
  /** Read supported native downloads and current managed installation state. */
  installOptions(): Promise<AgentInstallOptionsResult>
  /** Download the pinned runtime on explicit request. A lost response does not
   * cancel installation; refresh installOptions before making a new attempt.
   * requestId must be well-formed Unicode and satisfy the product schema's nonempty UTF-8 byte bound
   * published by AgentInstallParams; invalid IDs are rejected before sending.
   * The gateway retains this attempt ID with authenticated audit attribution.
   */
  install(agent: InstallableAgent, requestId: string): Promise<AgentInstallResult>
  /**
   * Allow or restore suppression of one harness's own hooks. The next launch
   * omits Nessa's suppression request when `enabled` is true. OpenCode is not
   * a value of `agent`. A lost response does not undo the change; list again
   * before retrying. `requestId` must be well-formed Unicode within the
   * product schema's bound. The gateway retains this attempt with
   * authenticated audit attribution. When enabled, a hook can deny or change
   * a tool call before Nessa asks.
   */
  setNativeHooks(
    agent: NativeHooksAgent,
    enabled: boolean,
    requestId: string,
  ): Promise<AgentsSetNativeHooksResult>
}

export function createAgentsApi(session: RpcRequester): AgentsApi {
  return {
    installOptions: async () =>
      agentInstallOptions(await session.request(ProductMethod.AgentsInstallOptions, {})),
    install: async (agent, requestId) =>
      agentInstallResult(
        await session.request(
          ProductMethod.AgentsInstall,
          { agent, requestId: agentInstallRequestId(requestId) },
          { atLeastMs: 46 * 60 * 1000 },
        ),
      ),
    list: async () => agentsList(await session.request(ProductMethod.AgentsList, {})),
    setNativeHooks: async (agent, enabled, requestId) =>
      agentsSetNativeHooks(
        await session.request(ProductMethod.AgentsSetNativeHooks, {
          agent,
          enabled,
          requestId: nativeHooksRequestId(requestId),
        }),
        { agent, requestId },
      ),
  }
}
