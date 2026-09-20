import {
  asImageAttachment,
  NessaAttachmentError,
  NessaConversationMutationError,
  type AttachmentBeginRefusal,
  type ConversationRejection,
  type NessaClient,
} from "@nessa/client"
import type { ConversationView } from "../../application/view"
import {
  AttachmentStagingError,
  ConversationUnavailableError,
  SubmissionRefusedError,
  type ConversationEffects,
  type SubmissionRefusal,
} from "../../application/ports"

/** Why the gateway declined to issue a ticket, as what the panel can do about it. */
function beginFailure(refusal: AttachmentBeginRefusal | undefined) {
  switch (refusal) {
    // Room frees as tickets are used or expire, and "temporarily" means it.
    case "attachment_capacity":
    case "temporarily_unavailable":
      return "busy" as const
    // Nothing was wrong with the bytes: the gateway could not keep or record
    // them, has no agent configured, or lost the conversation it was asked about.
    case "attachment_storage_unavailable":
    case "storage_unavailable":
    case "audit_unavailable":
    case "agent_not_configured":
    case "conversation_not_found":
      return "unavailable" as const
    case "invalid_request":
    case "unexpected":
    case undefined:
      return "rejected" as const
  }
}

/**
 * The client's staging codes, as what the panel can do about them.
 *
 * The gateway's verdicts on an image, and on an agent whose model takes none,
 * keep their meaning, because the tile says something different for each and
 * trying again would not change them. Everything that time or a second attempt
 * may cure is told apart from a refusal: no room for another upload (`busy`), a
 * transfer cut off or timed out (`interrupted`), and a spent or expired ticket,
 * storage or the audit record being away, or no answer at all (`unavailable`).
 * What is left — a size or digest the gateway disagreed with, an answer nobody
 * recognises, arguments the client itself would not send — is `rejected`.
 *
 * Exhaustive over the client's codes: a code added there does not compile here
 * until somebody decides what the panel says about it.
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
      case "image_input_unsupported":
        return new AttachmentStagingError("image-input-unsupported", error)
      case "temporarily_unavailable":
        return new AttachmentStagingError("busy", error)
      // `attachment_not_kept` belongs here too: the conversation let go of its
      // files while this one was arriving. Nothing is wrong with the file, and
      // uploading it again is the whole remedy.
      case "upload_interrupted":
      case "upload_timeout":
      case "attachment_not_kept":
      case "aborted":
        return new AttachmentStagingError("interrupted", error)
      case "unreachable":
      case "storage_unavailable":
      case "audit_unavailable":
      case "ticket_invalid":
        return new AttachmentStagingError("unavailable", error)
      case "begin_refused":
        return new AttachmentStagingError(beginFailure(error.refusal), error)
      case "size_mismatch":
      case "digest_mismatch":
      case "unexpected_response":
        return new AttachmentStagingError("rejected", error)
    }
  }
  // The client refusing its own arguments: these bytes cannot be described to
  // the gateway at all, which trying again will not change.
  if (error instanceof TypeError) return new AttachmentStagingError("rejected", error)
  return new AttachmentStagingError("unavailable", error)
}

const refusals: Record<ConversationRejection, SubmissionRefusal> = {
  image_input_unsupported: "image-input-unsupported",
  attachment_not_found: "attachment-not-found",
  attachment_unavailable: "attachment-unavailable",
  conversation_not_found: "conversation-not-found",
  conversation_capacity: "conversation-capacity",
  agent_not_configured: "agent-not-configured",
  invalid_request: "invalid-request",
}

/**
 * A send the gateway refused before admitting it, as the application's own typed
 * refusal. Anything else is passed on untouched: a lost acknowledgement is not a
 * refusal, and the store already knows what to do with one.
 */
function submissionFailure(error: unknown): unknown {
  if (!(error instanceof NessaConversationMutationError) || !error.rejection) return error
  const refused = new SubmissionRefusedError(refusals[error.rejection], error)
  // The client's message for a gateway with no agent names the remedy. Keep it.
  refused.message = error.message
  return refused
}

/**
 * How long to wait before offering the same ticket again after the gateway said
 * it had no room for another upload. That refusal is the one that does not spend
 * the ticket, and the gateway allows only a few uploads at once, holding each
 * slot until the image is normalized — so a short wait is usually all it takes.
 * Bounded: after the last wait the tile fails as `busy`, with its retry.
 */
export const BUSY_RETRY_DELAYS_MS: readonly number[] = [1000, 2000, 4000]

/** One application-scoped adapter; a disconnected transport never becomes a fake conversation. */
export function gatewayEffects(
  client: () => NessaClient | null,
  /** Resolves after `ms`. Composition supplies the real clock; tests resolve it by hand. */
  wait: (ms: number) => Promise<void>,
): ConversationEffects {
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
    async send(input) {
      try {
        return await api().send(input.conversationId, input.text, input.attachments, {
          executionId: input.executionId,
          requestId: input.actionId,
        })
      } catch (error) {
        throw submissionFailure(error)
      }
    },
    async steer(input) {
      try {
        return await api().steer(input.conversationId, input.text, input.attachments, {
          executionId: input.executionId,
          requestId: input.actionId,
        })
      } catch (error) {
        throw submissionFailure(error)
      }
    },
    async stageAttachment(conversationId, file, bytes, signal) {
      try {
        const attachments = connected().attachments
        const beginning = await attachments.begin(conversationId, file)
        // Either way the reference is the gateway's. The digest in `file` only
        // identified the upload; it is never what a message names.
        let stored
        if (beginning.state === "stored") {
          // The conversation already holds these bytes: nothing to send, no
          // ticket to send it with, and the reference comes with the answer.
          stored = beginning.stored
        } else {
          for (let attempt = 0; ; attempt++) {
            try {
              stored = await attachments.upload(
                beginning.ticket,
                { mimeType: file.mimeType, bytes },
                { signal },
              )
              break
            } catch (error) {
              const delay = BUSY_RETRY_DELAYS_MS[attempt]
              const busy =
                error instanceof NessaAttachmentError &&
                error.code === "temporarily_unavailable"
              // Only "no room just now" leaves the ticket unspent. Every other
              // failure ends this attempt; trying again means beginning again.
              if (!busy || delay === undefined || signal.aborted) throw error
              await wait(delay)
              // The session may have gone while waiting; say so rather than PUT.
              connected()
            }
          }
        }
        // Storage keeps any file. A message names only the four image
        // encodings, and this is where that is decided.
        const image = asImageAttachment(stored)
        if (!image) throw new AttachmentStagingError("unsupported-image")
        return image
      } catch (error) {
        throw error instanceof AttachmentStagingError ? error : stagingFailure(error)
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
