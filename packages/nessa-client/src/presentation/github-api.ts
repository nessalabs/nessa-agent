import { NessaGithubError, type GithubMethod } from "../application/github-error.js"
import type { RpcRequester } from "../application/session-port.js"
import { ProductMethod, type GithubConnection } from "../generated/product.js"
import { githubConnection } from "../protocol/github-validate.js"

/**
 * The gateway's GitHub sign-in through the Nessa GitHub App: read it, start
 * it, cancel the one waiting, or forget it. Each answers where the sign-in
 * stands now; none carries the token, which only the agents the gateway
 * opens are given.
 *
 * `connect` answers a code to enter on GitHub; the gateway waits for it on
 * its own, so read `status` again to see it become `connected` or end. Asking
 * to connect again while a code waits answers the same code. Nothing is
 * retried for you.
 */
export type GithubApi = {
  status(): Promise<GithubConnection>
  connect(): Promise<GithubConnection>
  cancel(): Promise<GithubConnection>
  /**
   * Forget the sign-in. GitHub keeps the authorization until the person
   * revokes it there; this gateway stops holding and giving out its token.
   */
  disconnect(): Promise<GithubConnection>
}

export function createGithubApi(session: RpcRequester): GithubApi {
  async function call(method: GithubMethod): Promise<GithubConnection> {
    try {
      return githubConnection(await session.request(method, {}))
    } catch (cause) {
      throw new NessaGithubError(method, cause)
    }
  }
  return {
    status: () => call(ProductMethod.GithubStatus),
    connect: () => call(ProductMethod.GithubConnect),
    cancel: () => call(ProductMethod.GithubCancel),
    disconnect: () => call(ProductMethod.GithubDisconnect),
  }
}
