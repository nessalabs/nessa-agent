import {
  asImageAttachment,
  IMAGE_ATTACHMENT_TYPES,
  MAX_IMAGE_ATTACHMENT_BYTES,
  NessaAttachmentError,
  NessaConversationControlError,
  NessaConversationMutationError,
  type AttachmentBeginRefusal,
  type ConversationErrorCode,
  type NessaClient,
  type StoredAttachment,
} from "@nessa/client"
import type { ConversationView } from "../../application/view"
import {
  AttachmentStagingError,
  ControlFailedError,
  ConversationUnavailableError,
  SubmissionRefusedError,
  type ControlOutcome,
  type ConversationEffects,
} from "../../application/ports"
import type { CommandFailure } from "../../model"

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
 * The gateway's codes, as the words this panel has for them. One table for
 * every conversation command, because a code means the same thing whichever
 * command met it, and this is the one place the wire's vocabulary is read.
 *
 * A code that is not here keeps the client's own sentence and reaches the panel
 * with no typed reason at all, which is what an unknown answer deserves: only
 * these change what the panel says or does about a failure.
 *
 * "Whichever command" is about meaning, not reach: `attachment_cleanup_unavailable`
 * means the same thing wherever it appears, and the gateway raises it only when
 * closing a conversation — `ConversationError::AttachmentRelease` is built in
 * the close path alone, from the arm where the close itself succeeded.
 */
const failures: Partial<Record<ConversationErrorCode, CommandFailure>> = {
  image_input_unsupported: "image-input-unsupported",
  attachment_not_found: "attachment-not-found",
  attachment_unavailable: "attachment-unavailable",
  attachment_cleanup_unavailable: "attachment-cleanup-unavailable",
  conversation_not_found: "conversation-not-found",
  conversation_capacity: "conversation-capacity",
  agent_not_configured: "agent-not-configured",
  agent_unsupported: "agent-unsupported",
  conversations_not_configured: "conversations-not-configured",
  agent_startup_deadline: "agent-startup-deadline",
  invalid_request: "invalid-request",
}

/**
 * A conversation, send, or steer that was refused rather than lost, as the
 * application's own typed refusal. Anything else is passed on untouched: a lost
 * acknowledgement is not a refusal, and the store already knows what to do with
 * one.
 *
 * Two things are refusals. The gateway's own pre-admission rejection, and the
 * client refusing the arguments — the client is the one boundary that validates
 * a message's images, and it does so before anything reaches the wire, so that
 * is as certain as a refusal gets: nothing was sent, and the draft comes back.
 * Either way the client's own sentence is kept, because it names what is wrong.
 */
function submissionFailure(error: unknown): unknown {
  const named =
    error instanceof NessaConversationMutationError && !error.uncertain && error.code
      ? failures[error.code]
      : undefined
  const refused =
    error instanceof TypeError
      ? new SubmissionRefusedError("invalid-request", error)
      : named
        ? new SubmissionRefusedError(named, error)
        : undefined
  if (!refused) return error
  refused.message = (error as Error).message
  return refused
}

/**
 * What the gateway said became of a control that failed.
 *
 * The review's selection state is read before the diagnostic code, because the
 * protocol says exactly that: it is "authoritative knowledge of whether the
 * reviewed option was selected", and "independent of the diagnostic error
 * code". A consumed option is the gateway stating the choice took effect, which
 * no error code beside it can withdraw; a pending one is it stating the choice
 * did not. Only with neither does the code decide, through the client's own
 * verdict on whether the command was rejected before anything was applied.
 */
function controlOutcome(error: NessaConversationControlError): ControlOutcome {
  if (error.permissionSelection === "consumed") return "applied"
  if (error.permissionSelection === "pending") return "refused"
  return error.uncertain ? "unknown" : "refused"
}

/**
 * A control the gateway answered with a reason, in the panel's words, and with
 * what became of it. Anything else is passed on untouched.
 *
 * Unlike a message, a control is translated whatever the client's `uncertain`
 * says, because the reason is worth passing on either way — the case that makes
 * the difference is `attachment_cleanup_unavailable`, a close that did happen
 * and whose cleanup did not, which the client can only report as uncertain.
 *
 * And the outcome travels even when the reason cannot. A review the gateway
 * reports as still pending is certainly not applied whatever code came with it,
 * and it sends an ordinary diagnostic code there rather than a dedicated one —
 * so discarding the outcome for want of a word would throw away the certain
 * half of the answer precisely where the panel has nothing else to go on.
 */
function controlFailure(error: unknown): unknown {
  if (!(error instanceof NessaConversationControlError)) return error
  const named = error.code ? failures[error.code] : undefined
  const outcome = controlOutcome(error)
  // Neither a word for the reason nor anything to say about the outcome that
  // the client's own sentence does not already say. Left exactly as it came.
  if (!named && outcome === "unknown") return error
  const failed = new ControlFailedError(named, outcome, error)
  // The client has one constant for every control — "did not return a
  // trustworthy acknowledgement" — which names neither the command nor its
  // cause. Kept as the text to fall back to, and the application decides where
  // that sentence is still the honest one.
  failed.message = error.message
  return failed
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
  /** The agent every conversation this panel creates runs on, as setup recorded
   * it. Asked once and remembered with the creation it was asked for, because
   * the answer comes from the host rather than from the conversation. Resolving
   * to nothing leaves the choice to the gateway's own default, which is what a
   * browser and a setup nobody finished both are. */
  chosenAgent: () => Promise<string | undefined> = async () => undefined,
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
      // A conversation that could not be opened is a message that was not sent,
      // and a control that never ran: the same refusals, translated the same way.
      //
      // The transport is checked before the agent is asked for, so a
      // disconnected panel still fails as a disconnected panel rather than
      // waiting on the host first. The handle that check produced is thrown
      // away rather than held across the await: asking the host is a round
      // trip, and a session that was retired inside it must not be the one
      // this create is sent over.
      api()
      const request = chosenAgent()
        .then((agent) => api().create({ conversationId, agent }))
        .catch((error: unknown) => {
          creations.delete(conversationId)
          throw submissionFailure(error)
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
      try {
        return (await api().reorder(conversationId, executionIds)).outcome
      } catch (error) {
        throw controlFailure(error)
      }
    },
    async remove(conversationId, executionId) {
      try {
        await api().remove(conversationId, executionId)
      } catch (error) {
        throw controlFailure(error)
      }
    },
    async answer(conversationId, executionId, permissionId, optionId) {
      try {
        await api().answer(conversationId, executionId, permissionId, optionId)
      } catch (error) {
        throw controlFailure(error)
      }
    },
    async cancel(conversationId, executionId, permissionId) {
      try {
        await api().cancel(
          conversationId,
          executionId,
          permissionId,
          "Dismissed from the conversation panel",
        )
      } catch (error) {
        throw controlFailure(error)
      }
    },
    async close(conversationId) {
      try {
        await api().close(conversationId)
      } catch (error) {
        throw controlFailure(error)
      }
    },
  }
}
