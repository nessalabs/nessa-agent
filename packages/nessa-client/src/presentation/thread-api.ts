import type { ConversationMessageReceipt } from "../generated/product.js"
import { passiveReadTiming, ProductMethod } from "../generated/product.js"
import { issueExecutionTarget } from "../application/execution-target.js"
import type { RpcRequester } from "../application/session-port.js"
import {
  conversationIdPattern,
  conversationPeek,
} from "../protocol/conversation-validate.js"
import type {
  ConversationApi,
  ConversationCreateOptions,
  ConversationSubmission,
} from "./conversation-api.js"

/** How a message reaches the thread. Passive notes are saved without model delivery. */
export type ThreadMessageMode = "queued" | "steering" | "passive"

/** Optional admission mode and stable identity for a deliberate same-message retry. */
export type SendMessageOptions = {
  /** Queued by default. Steering reports whether the provider injected or queued it. */
  mode?: ThreadMessageMode
  /** Reuse with the same thread, text, and mode after an uncertain result. */
  requestId?: string
}

/** A saved passive note or an admitted turn. Neither receipt proves model consumption. */
export type ThreadMessageReceipt = ConversationSubmission | ConversationMessageReceipt

/** Stop an exact execution, or capture the active execution through one passive read. */
export type StopThreadOptions = {
  /** Omitted captures the running turn once; a later turn is not substituted. */
  executionId?: string
  /** Stable action attribution. After uncertainty, inspect the thread before another action. */
  requestId?: string
}

/** Exact-turn stop acknowledgement, or an idle snapshot that sent no stop command. */
export type StopThreadResult =
  | Awaited<ReturnType<ConversationApi["stop"]>>
  | {
      /** The passive snapshot held no running execution; queued work was not targeted. */
      outcome: "no_active_turn"
    }

/** Thread commands use the existing conversation admission and retry owners. */
export function createThreadCommands(
  conversation: ConversationApi,
  newId: () => string,
  session: RpcRequester,
) {
  return {
    createThread: (options?: ConversationCreateOptions) => conversation.create(options),
    sendMessage: (
      threadId: string,
      text: string,
      options: SendMessageOptions = {},
    ): Promise<ThreadMessageReceipt> => {
      const mode = options.mode ?? "queued"
      if (mode !== "queued" && mode !== "steering" && mode !== "passive")
        throw new TypeError("Message mode must be queued, steering, or passive")
      const requestId = options.requestId ?? newId()
      if (mode === "passive")
        return conversation.message(threadId, text, "record_only", { requestId })
      return conversation.submit(
        conversation.prepare(threadId, text, [], [], {
          mode,
          requestId,
          executionId: requestId,
        }),
      )
    },
    stopThread: async (
      threadId: string,
      options: StopThreadOptions = {},
    ): Promise<StopThreadResult> => {
      if (!conversationIdPattern.test(threadId))
        throw new TypeError("Invalid conversation ID")
      // Capture caller input before waiting for the passive snapshot.
      const { executionId } = options
      const requestId = options.requestId ?? newId()
      if (executionId !== undefined && typeof executionId !== "string")
        throw new TypeError("Invalid execution ID")
      const id =
        executionId === undefined
          ? (conversationPeek(
              await session.request(
                ProductMethod.ConversationPeek,
                { conversationId: threadId },
                { atLeastMs: passiveReadTiming.minRequestTimeoutMs },
              ),
              threadId,
            ).executionId ?? undefined)
          : executionId
      if (id === undefined) return { outcome: "no_active_turn" }
      return conversation.stop(issueExecutionTarget(threadId, id), { requestId })
    },
  }
}
