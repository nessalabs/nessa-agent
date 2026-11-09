/**
 * Putting one image's bytes where a conversation holds them, and answering the
 * reference a message names it by — one owner for every surface that sends
 * images: the panel stages each image as it is attached, the desktop window as
 * its message is sent.
 */
import {
  asImageAttachment,
  IMAGE_ATTACHMENT_TYPES,
  MAX_IMAGE_ATTACHMENT_BYTES,
  NessaAttachmentError,
  type AttachmentApi,
  type AttachmentBeginRefusal,
  type StoredAttachment,
} from "@nessa/client"
import {
  AttachmentStagingError,
  ConversationUnavailableError,
  type UploadedFile,
} from "../../application/ports"
import type { ImageReference } from "../../model"

/**
 * How long to wait before offering the same ticket again after the gateway said
 * it had no room for another upload. That refusal is the one that does not spend
 * the ticket, and the gateway allows only a few uploads at once, holding each
 * slot until the image is normalized — so a short wait is usually all it takes.
 * Bounded: after the last wait the tile fails as `busy`, with its retry.
 */
export const BUSY_RETRY_DELAYS_MS: readonly number[] = [1000, 2000, 4000]

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
    case "image_input_unsupported":
      return "image-input-unsupported" as const
    // Deleted, here or on another surface: nothing is ever added to it again.
    case "conversation_deleted":
      return "conversation-deleted" as const
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
      // Two of these are the gateway's doing and not the file's:
      // `attachment_not_kept` is a conversation that let go of its files while
      // this one was arriving, and `upload_unresolved` is the gateway's own work
      // stopping without an answer. Uploading again is the whole remedy for both.
      case "upload_interrupted":
      case "upload_timeout":
      case "attachment_not_kept":
      case "upload_unresolved":
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

/**
 * Upload `bytes` into conversation `conversationId`, which must already exist
 * on the gateway, and answer the reference a message names the image by.
 *
 * `attachments` is asked for the client's attachment API each time it is
 * used, so a session that went while this waited fails as the session gone
 * rather than uploading on a client nobody holds. Rejects with
 * {@link AttachmentStagingError}, whatever went wrong.
 */
export async function stageImage(
  attachments: () => Pick<AttachmentApi, "begin" | "upload">,
  conversationId: string,
  file: UploadedFile,
  bytes: Blob,
  {
    signal,
    wait,
  }: {
    /** Aborted when the image stops being wanted; the upload stops with it. */
    signal: AbortSignal
    /** Resolves after `ms`: the busy retries' clock. */
    wait: (ms: number) => Promise<void>
  },
): Promise<ImageReference> {
  try {
    // No longer wanted while its bytes were read: no ticket is taken for it.
    if (signal.aborted) throw new AttachmentStagingError("interrupted")
    const beginning = await attachments().begin(conversationId, file)
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
          stored = await attachments().upload(
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
}
