import { ProductMethod, type ProductSessionReady } from "../protocol/product-types.js"
import { assertProductSessionReady } from "../protocol/validate.js"
import type { RpcRequester } from "../application/session-port.js"

/** Current authenticated identity and credential restrictions. This is a snapshot. */
export type AuthApi = {
  /** Query auth.session for current identity and credential restrictions. Requires a connected product session; this snapshot is not a lasting authorization decision. */
  session: () => Promise<ProductSessionReady>
}

export function createAuthApi(session: RpcRequester): AuthApi {
  return {
    session: async () =>
      assertProductSessionReady(await session.request(ProductMethod.AuthSession, {})),
  }
}
