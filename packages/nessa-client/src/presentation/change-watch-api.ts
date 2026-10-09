import type { RpcRequester } from "../application/session-port.js"
import { ProductMethod, passiveReadTiming } from "../generated/product.js"
import type {
  ConversationWatchCatalogueParams,
  ConversationWatchRecordsParams,
  ConversationWatchResult,
  ConversationUnwatchResult,
} from "../generated/product.js"
import { validChangeWatchId, watchResult } from "../protocol/change-watch-validate.js"
import { validPositiveReadEpoch } from "../protocol/passive-read-validate.js"
import { validRecordReceiverId } from "../protocol/record-read-validate.js"
import { conversationIdPattern } from "../protocol/conversation-validate.js"

/** Advisory notifications on this connection only. Reconnect loses registrations;
 * callers must register again and perform a final authorized head recheck. These
 * calls do not start a reader, fold, scheduler or automatic replay. */
export interface ChangeWatchApi {
  /** Register one conversation target. Resolve after the server's delivery acknowledgement, then recheck its head before trusting subsequent hints. */
  records(params: ConversationWatchRecordsParams): Promise<ConversationWatchResult>
  /** Register this receiver's catalogue. The server selects its current authorized owner. */
  catalogue(params: ConversationWatchCatalogueParams): Promise<ConversationWatchResult>
  /**
   * Register one conversation the authenticated owner session already may read.
   * Sends no receiver. A paired device uses {@link ChangeWatchApi.records}.
   */
  ownedRecords(conversationId: string): Promise<ConversationWatchResult>
  /**
   * Register the authenticated owner's catalogue. Sends no receiver. A paired
   * device uses {@link ChangeWatchApi.catalogue}.
   */
  ownedCatalogue(): Promise<ConversationWatchResult>
  /** Remove an ID minted on this connection. Exact repeat removal is idempotent; foreign IDs refuse. The ACK confirms interest cancellation; admitted authority/frame ownership can remain. Immediate replacement may refuse until that original work completes. */
  unwatch(watchId: string): Promise<ConversationUnwatchResult>
}

function binding(params: { receiverId?: string; accessEpoch?: string }): void {
  if (
    params.receiverId === undefined ||
    params.accessEpoch === undefined ||
    !validRecordReceiverId(params.receiverId) ||
    !validPositiveReadEpoch(params.accessEpoch)
  )
    throw new TypeError("Invalid watch receiver or epoch")
}

/** Use the existing managed session and its single wire dispatcher. No request is replayed during recovery. */
export function createChangeWatchApi(session: RpcRequester): ChangeWatchApi {
  const options = { atLeastMs: passiveReadTiming.minRequestTimeoutMs }
  return {
    records: async (params) => {
      binding(params)
      if (!conversationIdPattern.test(params.conversationId))
        throw new TypeError("Invalid conversation ID")
      return watchResult(
        await session.request(ProductMethod.ConversationWatchRecords, params, options),
      )
    },
    catalogue: async (params) => {
      binding(params)
      return watchResult(
        await session.request(ProductMethod.ConversationWatchCatalogue, params, options),
      )
    },
    ownedRecords: async (conversationId) => {
      if (!conversationIdPattern.test(conversationId))
        throw new TypeError("Invalid conversation ID")
      return watchResult(
        await session.request(
          ProductMethod.ConversationWatchRecords,
          { conversationId },
          options,
        ),
      )
    },
    ownedCatalogue: async () =>
      watchResult(
        await session.request(ProductMethod.ConversationWatchCatalogue, {}, options),
      ),
    unwatch: async (watchId) => {
      if (!validChangeWatchId(watchId)) throw new TypeError("Invalid change watch ID")
      const result = watchResult(
        await session.request(ProductMethod.ConversationUnwatch, { watchId }, options),
      )
      if (result.watchId !== watchId)
        throw new TypeError("Mismatched change watch removal")
      return result
    },
  }
}
