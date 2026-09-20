import type { NessaClient } from "@nessa/client"
import type { ConversationView } from "../../application/view"
import {
  ConversationUnavailableError,
  type ConversationEffects,
} from "../../application/ports"

/** One application-scoped adapter; a disconnected transport never becomes a fake conversation. */
export function gatewayEffects(client: () => NessaClient | null): ConversationEffects {
  const creations = new Map<string, Promise<{ conversationId: string }>>()
  const reads = new Map<string, Promise<ConversationView>>()
  const api = () => {
    const current = client()
    if (
      !current ||
      (current.connectionState && current.connectionState.status !== "connected")
    )
      throw new ConversationUnavailableError()
    return current.conversation
  }
  return {
    create(conversationId) {
      const existing = creations.get(conversationId)
      if (existing) return existing
      const request = api()
        .create({ conversationId })
        .catch((error) => {
          creations.delete(conversationId)
          throw error
        })
      creations.set(conversationId, request)
      return request
    },
    read(conversationId) {
      // Opaque revisions have no numeric ordering. Serialize reads, including
      // refreshes after mutations, so server snapshots cannot overtake each other.
      const previous = reads.get(conversationId)
      const request = (previous ?? Promise.resolve())
        .catch(() => undefined)
        .then(() => api().read(conversationId))
      reads.set(conversationId, request)
      const release = () => {
        if (reads.get(conversationId) === request) reads.delete(conversationId)
      }
      void request.then(release, release)
      return request
    },
    send: (input) =>
      api().send(input.conversationId, input.text, [], {
        executionId: input.executionId,
        requestId: input.actionId,
      }),
    steer: (input) =>
      api().steer(input.conversationId, input.text, [], {
        executionId: input.executionId,
        requestId: input.actionId,
      }),
    async reorder(conversationId, executionIds) {
      return (await api().reorder(conversationId, executionIds)).outcome
    },
    async remove(conversationId, executionId) {
      await api().remove(conversationId, executionId)
    },
    async answer(conversationId, executionId, permissionId, optionId) {
      await api().answer(conversationId, executionId, permissionId, optionId)
    },
    async cancel(conversationId, executionId, permissionId) {
      await api().cancel(
        conversationId,
        executionId,
        permissionId,
        "Dismissed from the conversation panel",
      )
    },
    async close(conversationId) {
      await api().close(conversationId)
    },
  }
}
