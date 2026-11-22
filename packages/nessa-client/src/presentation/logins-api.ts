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
 *
 * Codex can instead be handed a ChatGPT account's token, when the gateway
 * keeps them (`status.chatgpt`). `chatgptSignIn` starts a sign-in and
 * answers at once with `signIn.state` waiting: open `signIn.authorizeUrl` on
 * the gateway's own computer, which the browser comes back to, and read
 * `status` until it is idle, its `last` saying how it ended.
 */
export type LoginsApi = {
  status(): Promise<LoginsStatus>
  save(agent: LoginAgent, login: string): Promise<LoginsStatus>
  remove(agent: LoginAgent): Promise<LoginsStatus>
  setDevice(environment: string, on: boolean): Promise<LoginsStatus>
  /** Sign in a new ChatGPT account, or `accountId` again. */
  chatgptSignIn(accountId?: string): Promise<LoginsStatus>
  /** Stop the ChatGPT sign-in waiting, if one is. */
  chatgptCancel(): Promise<LoginsStatus>
  /** Have Codex use `accountId`, or its API key again when `null`. */
  chatgptUse(accountId: string | null): Promise<LoginsStatus>
  /** Forget `accountId`, revoking it at OpenAI when that answers. */
  chatgptRemove(accountId: string): Promise<LoginsStatus>
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
    chatgptSignIn: (accountId) =>
      call(
        ProductMethod.LoginsChatgptSignIn,
        accountId === undefined ? {} : { accountId },
      ),
    chatgptCancel: () => call(ProductMethod.LoginsChatgptCancel, {}),
    chatgptUse: (accountId) => call(ProductMethod.LoginsChatgptUse, { accountId }),
    chatgptRemove: (accountId) => call(ProductMethod.LoginsChatgptRemove, { accountId }),
  }
}
