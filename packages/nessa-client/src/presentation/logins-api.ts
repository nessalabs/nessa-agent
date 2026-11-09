import { NessaLoginsError, type LoginsMethod } from "../application/logins-error.js"
import type { RpcRequester } from "../application/session-port.js"
import {
  ProductMethod,
  type LoginAgent,
  type LoginsStatus,
} from "../generated/product.js"
import { loginsStatus } from "../protocol/logins-validate.js"

/**
 * The agents' saved logins the gateway keeps in its vault, and which paired
 * machines may be handed them when an agent starts there. A login is never
 * given back: `status` says only which agents have one. SSH hosts and Boat
 * workspaces may always be handed them; a paired machine only once its
 * switch is on. Every method answers the status after it. Nothing is
 * retried for you.
 */
export type LoginsApi = {
  status(): Promise<LoginsStatus>
  save(agent: LoginAgent, login: string): Promise<LoginsStatus>
  remove(agent: LoginAgent): Promise<LoginsStatus>
  setDevice(environment: string, on: boolean): Promise<LoginsStatus>
}

export function createLoginsApi(session: RpcRequester): LoginsApi {
  async function call(
    method: LoginsMethod,
    params: Record<string, unknown>,
  ): Promise<LoginsStatus> {
    try {
      return loginsStatus(await session.request(method, params))
    } catch (cause) {
      throw new NessaLoginsError(method, cause)
    }
  }
  return {
    status: () => call(ProductMethod.LoginsStatus, {}),
    save: (agent, login) => call(ProductMethod.LoginsSave, { agent, login }),
    remove: (agent) => call(ProductMethod.LoginsRemove, { agent }),
    setDevice: (environment, on) =>
      call(ProductMethod.LoginsSetDevice, { environment, on }),
  }
}
