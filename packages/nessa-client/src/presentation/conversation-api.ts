import type { EchoResult } from "../protocol/index.js"
import { Method } from "../generated/catalog.js"
import { assertEchoResult } from "../protocol/validate.js"
import type { RpcRequester } from "../application/session-port.js"

/** Typed `conversation.*` RPC namespace on a connected client. */
export type ConversationApi = {
  /** Send text through the authenticated conversation.write action and receive it back. This temporary handler does not generate model output. */
  echo: (text: string) => Promise<EchoResult>
}

export function createConversationApi(session: RpcRequester): ConversationApi {
  return {
    echo: async (text: string) =>
      assertEchoResult(await session.request(Method.ConversationEcho, { text })),
  }
}
