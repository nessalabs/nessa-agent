import type { RpcRequester } from "../application/session-port.js"
import { ProductMethod } from "../generated/product.js"

/**
 * The apps the person always allows: every MCP server whose app was answered
 * "Always allow this app". Such an app's messages go as the person without a
 * review until it is untrusted here. Trust is kept gateway-wide, so only the
 * owner's own surfaces may read or change it.
 */
export type AppsApi = {
  /** The servers whose apps are always allowed, in name order. */
  trusted(): Promise<readonly string[]>
  /**
   * Stop always allowing `server`'s app: its next message is reviewed again.
   * Answers whether it was always allowed.
   */
  untrust(server: string, requestId: string): Promise<boolean>
}

const MAX_TRUSTED = 256

export function createAppsApi(session: RpcRequester): AppsApi {
  return {
    trusted: async () => {
      const answer = await session.request(ProductMethod.AppsTrusted, {})
      if (
        !isRecord(answer) ||
        Object.keys(answer).length !== 1 ||
        !Array.isArray(answer.servers) ||
        answer.servers.length > MAX_TRUSTED ||
        !answer.servers.every((server) => typeof server === "string" && server !== "")
      )
        throw new Error("Invalid trusted apps response")
      return answer.servers as string[]
    },
    untrust: async (server, requestId) => {
      const answer = await session.request(ProductMethod.AppsUntrust, {
        requestId,
        server,
      })
      if (
        !isRecord(answer) ||
        Object.keys(answer).length !== 2 ||
        answer.requestId !== requestId ||
        typeof answer.applied !== "boolean"
      )
        throw new Error("Invalid untrust app response")
      return answer.applied
    },
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
}
