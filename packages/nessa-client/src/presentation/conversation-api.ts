import type { EchoResult } from "../protocol/index.js"
import { Method } from "../generated/catalog.js"
import { assertEchoResult } from "../protocol/validate.js"
import type { WireSession } from "../transport/wire-session.js"

/** Typed `conversation.*` RPC namespace on a connected client. */
export type ConversationApi = {
  echo: (text: string) => Promise<EchoResult>
}

export function createConversationApi(session: WireSession): ConversationApi {
  return {
    echo: async (text: string) =>
      assertEchoResult(await session.request(Method.ConversationEcho, { text })),
  }
}
