import type { HealthResult } from "../protocol/index.js"
import { Method } from "../generated/catalog.js"
import { assertHealthResult } from "../protocol/validate.js"
import type { RpcRequester } from "../application/session-port.js"

/** Typed `server.*` RPC namespace on a connected client. */
export type ServerApi = {
  /** Read gateway health. Product sessions require current server.read authorization. */
  health: () => Promise<HealthResult>
}

export function createServerApi(session: RpcRequester): ServerApi {
  return {
    health: async () =>
      assertHealthResult(await session.request(Method.ServerHealth, {})),
  }
}
