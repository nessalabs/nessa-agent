import type { HealthResult } from "../protocol/index.js"
import { Method } from "../generated/catalog.js"
import { assertHealthResult } from "../protocol/validate.js"
import type { WireSession } from "../transport/wire-session.js"

/** Typed `server.*` RPC namespace on a connected client. */
export type ServerApi = {
  health: () => Promise<HealthResult>
}

export function createServerApi(session: WireSession): ServerApi {
  return {
    health: async () =>
      assertHealthResult(await session.request(Method.ServerHealth, {})),
  }
}
