import { validPositiveReadEpoch } from "../protocol/passive-read-validate.js"
import type { RpcRequester } from "../application/session-port.js"
import { passiveReadTiming, ProductMethod } from "../generated/product.js"
import type {
  ConversationRecordsHeadResult,
  RecordPageRequest,
} from "../generated/product.js"
import {
  recordHead,
  recordPage,
  validRecordPageRequest,
  validRecordReceiverId,
  type DecodedRecordPage,
} from "../protocol/record-read-validate.js"
import { conversationIdPattern } from "../protocol/conversation-validate.js"

/** Authorized, fixed-target reads of committed physical conversation records. */
export type RecordReadApi = {
  /** Discover the actual scope and committed head for an authenticated receiver selector. Recheck after each fixed pass to discover later commits. Authorization is fresh for each call. */
  head: (
    conversationId: string,
    receiverId: string,
    accessEpoch: string,
  ) => Promise<ConversationRecordsHeadResult>
  /** Read one bounded page after a durable downloaded checkpoint. A lost reply may be retried with the same request; the receiver must validate and commit downloaded records and progress atomically. */
  page: (
    conversationId: string,
    accessEpoch: string,
    request: RecordPageRequest,
  ) => Promise<DecodedRecordPage>
}

function checkedConversationId(value: string): string {
  if (!conversationIdPattern.test(value)) throw new TypeError("Invalid conversation ID")
  return value
}

/** Build record calls over the existing product session without opening an Agent. */
export function createRecordReadApi(session: RpcRequester): RecordReadApi {
  return {
    head: async (conversation, receiverId, accessEpoch) => {
      const id = checkedConversationId(conversation)
      if (!validPositiveReadEpoch(accessEpoch) || !validRecordReceiverId(receiverId))
        throw new TypeError("Invalid record scope or binding epoch")
      const response = await session.request(
        ProductMethod.ConversationRecordsHead,
        { conversationId: id, accessEpoch, receiverId },
        { atLeastMs: passiveReadTiming.minRequestTimeoutMs },
      )
      return recordHead(response)
    },
    page: async (conversation, accessEpoch, request) => {
      const id = checkedConversationId(conversation)
      if (!validPositiveReadEpoch(accessEpoch) || !validRecordPageRequest(request)) {
        throw new TypeError("Invalid record page request")
      }
      const response = await session.request(
        ProductMethod.ConversationRecordsPage,
        { conversationId: id, accessEpoch, request },
        { atLeastMs: passiveReadTiming.minRequestTimeoutMs },
      )
      return recordPage(response)
    },
  }
}
