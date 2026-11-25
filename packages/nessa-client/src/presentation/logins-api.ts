import { NessaLoginsError, type LoginsMethod } from "../application/logins-error.js"
import type { RpcRequester } from "../application/session-port.js"
import {
  ProductMethod,
  type LoginAgent,
  type LoginsStatus,
} from "../generated/product.js"
import { loginsStatus } from "../protocol/logins-validate.js"

/**
 * The agents' saved logins the gateway keeps in its vault, handed to agents
 * on the cloud boxes it makes; its own machine never is. A login is never
 * given back: `status` says only which agents have one. Every method
 * answers the status after it. Nothing is retried for you.
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
