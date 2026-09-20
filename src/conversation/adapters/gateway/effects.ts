import { NessaAttachmentError, type NessaClient } from "@nessa/client"
import type { ConversationView } from "../../application/view"
import {
  AttachmentStagingError,
  ConversationUnavailableError,
  type ConversationEffects,
} from "../../application/ports"

/**
 * The client's staging codes, as what the panel can do about them.
 *
 * The gateway's two verdicts on an image keep their meaning, because the tile
 * says something different for each. A spent or expired ticket, storage or the
 * audit record being away, and no answer at all are `unavailable`: nothing was
 * wrong with the bytes, and beginning again may work. Everything else — a size
 * or digest the gateway disagreed with, a refused begin, an answer nobody
 * recognises, arguments the client itself would not send — is `rejected`.
 */
function stagingFailure(error: unknown): AttachmentStagingError {
  if (error instanceof ConversationUnavailableError)
    return new AttachmentStagingError("unavailable", error)
  if (error instanceof NessaAttachmentError) {
    switch (error.code) {
      case "unsupported_image":
        return new AttachmentStagingError("unsupported-image", error)
      case "image_too_large":
        return new AttachmentStagingError("too-large", error)
      case "unreachable":
      case "storage_unavailable":
      case "audit_unavailable":
      case "ticket_invalid":
        return new AttachmentStagingError("unavailable", error)
      case "size_mismatch":
      case "digest_mismatch":
      case "begin_refused":
      case "unexpected_response":
        return new AttachmentStagingError("rejected", error)
    }
  }
  // The client refusing its own arguments: these bytes cannot be described to
  // the gateway at all, which trying again will not change.
  if (error instanceof TypeError) return new AttachmentStagingError("rejected", error)
  return new AttachmentStagingError("unavailable", error)
}

/** One application-scoped adapter; a disconnected transport never becomes a fake conversation. */
export function gatewayEffects(client: () => NessaClient | null): ConversationEffects {
  const creations = new Map<string, Promise<{ conversationId: string }>>()
  const reads = new Map<string, Promise<ConversationView>>()
  const connected = () => {
    const current = client()
    if (
      !current ||
      (current.connectionState && current.connectionState.status !== "connected")
    )
      throw new ConversationUnavailableError()
    return current
  }
  const api = () => connected().conversation
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
      api().send(input.conversationId, input.text, input.attachments, {
        executionId: input.executionId,
        requestId: input.actionId,
      }),
    steer: (input) =>
      api().steer(input.conversationId, input.text, input.attachments, {
        executionId: input.executionId,
        requestId: input.actionId,
      }),
    async stageAttachment(conversationId, file, bytes) {
      try {
        const attachments = connected().attachments
        const beginning = await attachments.begin(conversationId, file)
        // The conversation already holds these bytes: nothing to send, no
        // ticket to send it with, and the reference comes with the answer.
        if (beginning.state === "stored") return beginning.image
        // Either way the reference is the gateway's. The digest in `file` only
        // identified the upload; it is never what a message names.
        return await attachments.upload(beginning.ticket, {
          mimeType: file.mimeType,
          bytes,
        })
      } catch (error) {
        throw stagingFailure(error)
      }
    },
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
