import type { RpcRequester } from "../application/session-port.js"
import { passiveReadTiming, ProductMethod } from "../generated/product.js"
import type {
  ConversationCatalogueHeadParams,
  ConversationCatalogueHeadResult,
  ConversationCatalogueManifestParams,
  ConversationCatalogueManifestResult,
  ConversationCatalogueResolveParams,
} from "../generated/product.js"
import {
  catalogueHead,
  catalogueManifest,
  catalogueResolved,
  checkedCatalogueShape,
  type DecodedCatalogueResolve,
} from "../protocol/catalogue-read-validate.js"

/** Current owner catalogue reads over the authenticated product session. */
export interface CatalogueReadApi {
  /** Discover the trusted owner stream, source incarnation, and current head. Fresh receiver authority is required on every call. */
  head(params: ConversationCatalogueHeadParams): Promise<ConversationCatalogueHeadResult>
  /** Read one finite creation-order manifest. Retry lost replies with the saved pass; commit only after sync-engine validates the echoed request. */
  manifest(
    params: ConversationCatalogueManifestParams,
  ): Promise<ConversationCatalogueManifestResult>
  /** Resolve the current value or retained deletion at a descriptor. Bytes are decoded and bounded; sync-engine owns key/revision/deletion validation before atomic cache commit. */
  resolve(params: ConversationCatalogueResolveParams): Promise<DecodedCatalogueResolve>
}

/** Build passive catalogue calls without opening a conversation or provider. */
export function createCatalogueReadApi(session: RpcRequester): CatalogueReadApi {
  return {
    head: async (params) => {
      checkedCatalogueShape(params, "ConversationCatalogueHeadParams")
      const response = catalogueHead(
        await session.request(ProductMethod.ConversationCatalogueHead, params, {
          atLeastMs: passiveReadTiming.minRequestTimeoutMs,
        }),
      )
      return response
    },
    manifest: async (params) => {
      checkedCatalogueShape(params, "ConversationCatalogueManifestParams")
      const response = catalogueManifest(
        await session.request(ProductMethod.ConversationCatalogueManifest, params, {
          atLeastMs: passiveReadTiming.minRequestTimeoutMs,
        }),
      )
      return response
    },
    resolve: async (params) => {
      checkedCatalogueShape(params, "ConversationCatalogueResolveParams")
      const response = catalogueResolved(
        await session.request(ProductMethod.ConversationCatalogueResolve, params, {
          atLeastMs: passiveReadTiming.minRequestTimeoutMs,
        }),
      )
      return response
    },
  }
}
