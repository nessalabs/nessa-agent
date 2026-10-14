import type { PreparedSubmission } from "@nessa/client"
import type { Receipt, Turn } from "../../model"

export type PreparedOperation = {
  readonly prepared: PreparedSubmission
  stopRequested: boolean
  inflight: boolean
}

/** Retains exact retry inputs only while their local turn is still retryable. */
export class PreparedOperationTable {
  private readonly conversations = new Map<string, Map<string, PreparedOperation>>()

  get(conversationId: string, executionId: string): PreparedOperation | undefined {
    return this.conversations.get(conversationId)?.get(executionId)
  }

  set(conversationId: string, executionId: string, operation: PreparedOperation): void {
    let operations = this.conversations.get(conversationId)
    if (!operations) this.conversations.set(conversationId, (operations = new Map()))
    operations.set(executionId, operation)
  }

  delete(conversationId: string, executionId: string): void {
    const operations = this.conversations.get(conversationId)
    operations?.delete(executionId)
    if (!operations?.size) this.conversations.delete(conversationId)
  }

  prune(
    conversations: readonly {
      id: string
      turns: readonly {
        from: Turn["from"]
        executionId?: string
        receipt?: Receipt
      }[]
    }[],
  ): void {
    for (const [conversationId, operations] of this.conversations) {
      const current = conversations.find(
        (conversation) => conversation.id === conversationId,
      )
      for (const [executionId, operation] of operations) {
        const turn = current?.turns.find(
          (turn) => turn.from === "user" && turn.executionId === executionId,
        )
        if (!turn || (turn.receipt !== "unknown" && !operation.inflight))
          operations.delete(executionId)
      }
      if (!operations.size) this.conversations.delete(conversationId)
    }
  }
}
