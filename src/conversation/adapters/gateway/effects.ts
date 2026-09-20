import {
  asImageAttachment,
  IMAGE_ATTACHMENT_TYPES,
  MAX_IMAGE_ATTACHMENT_BYTES,
  NessaAttachmentError,
  NessaConversationMutationError,
  type AttachmentBeginRefusal,
  type ConversationRejection,
  type NessaClient,
  type StoredAttachment,
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
 * The client's staging codes, as one of the reasons a tile can show — see
 * {@link UploadFailure} for what each of those means.
 *
 * The translation is what this owns: the gateway's verdicts on an image and on
 * an agent keep their own meaning, and everything time or a second attempt may
 * cure is told apart from a refusal. Exhaustive over the client's codes, so a
 * code added there does not compile here until somebody decides what the panel
 * says about it.
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

/**
 * Why a file the conversation now holds is not an image a message may name.
 *
 * Three separate facts, told apart because the tile offers a retry for one of
 * them and says something different for each: an encoding no message names
 * (`unsupported-image` — a stored PDF, or an image the gateway left as HEIC),
 * one of the four encodings over the protocol's per-image bound (`too-large`),
 * and a reference malformed in some other way, which is the gateway answering
 * something this window cannot use (`rejected`).
 */
function storedImageRefusal(stored: StoredAttachment): AttachmentStagingError {
  if (!(IMAGE_ATTACHMENT_TYPES as readonly string[]).includes(stored.mimeType))
    return new AttachmentStagingError("unsupported-image")
  if (stored.size > MAX_IMAGE_ATTACHMENT_BYTES)
    return new AttachmentStagingError("too-large")
  return new AttachmentStagingError("rejected")
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
 * A send that was refused rather than lost, as the application's own typed
 * refusal. Anything else is passed on untouched: a lost acknowledgement is not a
 * refusal, and the store already knows what to do with one.
 *
 * Two things are refusals. The gateway's own pre-admission rejection, and the
 * client refusing the arguments — the client is the one boundary that validates
 * a message's images, and it does so before anything reaches the wire, so that
 * is as certain as a refusal gets: nothing was sent, and the draft comes back.
 * Either way the client's own sentence is kept, because it names what is wrong.
 */
function submissionFailure(error: unknown): unknown {
  const refused =
    error instanceof TypeError
      ? new SubmissionRefusedError("invalid-request", error)
      : error instanceof NessaConversationMutationError && error.rejection
        ? new SubmissionRefusedError(refusals[error.rejection], error)
        : undefined
  if (!refused) return error
  refused.message = (error as Error).message
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
        // encodings, within the protocol's bound, and this is where that is
        // decided — separately for each of those, because a readable 6 MiB PNG
        // and a stored PDF are not the same news for the tile.
        const image = asImageAttachment(stored)
        if (!image) throw storedImageRefusal(stored)
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
