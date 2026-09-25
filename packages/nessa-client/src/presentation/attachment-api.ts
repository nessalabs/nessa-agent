import {
  beginRefusal,
  NessaAttachmentError,
  UPLOAD_DEADLINE_MS,
  uploadRefusal,
  type AttachmentUploadReply,
  type AttachmentUploadTransport,
  type UploadTimer,
} from "../application/attachment-upload.js"
import { NessaRpcError } from "../application/rpc-error.js"
import type { RpcRequester } from "../application/session-port.js"
import { ProductMethod } from "../generated/product.js"
import {
  attachmentBegin,
  MAX_UPLOAD_BYTES,
  storedAttachment,
  validDigest,
  validMediaType,
  validSize,
  type StoredAttachment,
} from "../protocol/attachment-validate.js"
import { conversationIdPattern } from "../protocol/conversation-validate.js"
import type { ConversationActionOptions } from "./conversation-api.js"

/** The bytes an upload is for: what they hash to, what they are, how many there are. */
export type AttachmentDescription = {
  /** `sha256:` and 64 lowercase hexadecimal digits, over exactly the bytes to upload. */
  digest: string
  /** Lowercase media type without parameters, at most 127 characters. */
  mimeType: string
  /** Exact length in bytes, 1 to 64 MiB. */
  size: number
}

/** The conversation already holds these bytes, or here is the ticket to upload them once. */
export type AttachmentBeginning =
  | {
      requestId: string
      state: "stored"
      /**
       * What the conversation holds them as. It may differ from the description
       * given, because the gateway normalizes images. Of any media type; see
       * `asImageAttachment` for whether a message may refer to it.
       */
      stored: StoredAttachment
    }
  | {
      requestId: string
      state: "upload_required"
      /** Secret, single-use, and bound to these bytes. Pass it to `upload`; never log it. */
      ticket: string
      /** Unix milliseconds after which the ticket is refused. */
      expiresAtMs: number
    }

/**
 * Put a file where a conversation holds it, and learn what it holds it as.
 *
 * Bytes never ride in a conversation command. Begin over the socket, which knows
 * who is asking; upload the original bytes over HTTP with the ticket that answer
 * carried. Storage takes any media type up to 64 MiB and both steps answer with
 * a {@link StoredAttachment}. That reference is the gateway's, not yours: it
 * converts, scales, and compresses images to what the selected model takes, so
 * the stored digest, media type, and size may all differ from what was uploaded.
 *
 * Whether a message may refer to a stored file is a separate, explicit step:
 * `asImageAttachment(stored)` answers the `ImageAttachment` to pass to
 * `conversation.send`, or undefined for anything that is not one of the four
 * image encodings. Never send a digest you computed.
 *
 * Every failure is a {@link NessaAttachmentError} with a code. Nothing is
 * retried for you.
 */
export type AttachmentApi = {
  /**
   * Ask to upload into a conversation that already exists on the gateway
   * (`conversation.create` is idempotent; call it first).
   * @param conversationId - Canonical lowercase UUID of that conversation.
   * @param file - Digest, media type, and size of exactly the bytes to upload.
   * Any media type up to 64 MiB; what becomes of it is the gateway's decision.
   * @param options - Optional caller-managed action identity.
   * @returns `stored` with what the conversation holds when nothing further is
   * needed, otherwise a ticket.
   * @throws TypeError for arguments the gateway would refuse;
   * NessaAttachmentError `begin_refused` when it did refuse — its `refusal` is
   * the gateway's own reason, of which `attachment_capacity` and
   * `temporarily_unavailable` pass with time — `unreachable` when no answer
   * arrived, `unexpected_response` for an answer that contradicts itself.
   */
  begin: (
    conversationId: string,
    file: AttachmentDescription,
    options?: ConversationActionOptions,
  ) => Promise<AttachmentBeginning>
  /**
   * Send the bytes a ticket was issued for.
   * @param ticket - From `begin`. Spent by this call unless it fails as
   * `temporarily_unavailable`, after which the same ticket may be tried again.
   * @param file - The same media type given to `begin`, and the bytes themselves.
   * @param options - `signal` abandons the request, which then fails as `aborted`.
   * @returns What the gateway stored them as.
   * @throws NessaAttachmentError with any upload-route code (see
   * `AttachmentFailureCode`), `upload_timeout` when no answer arrived within
   * this client's three-minute deadline, `aborted`, `unreachable` when the
   * request failed without an answer, or `unexpected_response` — including a
   * success whose reference is malformed.
   */
  upload: (
    ticket: string,
    file: { mimeType: string; bytes: Blob },
    options?: { signal?: AbortSignal },
  ) => Promise<StoredAttachment>
}

const ticketPattern = /^[0-9a-f]{64}$/
const utf8 = new TextEncoder()

/**
 * One PUT that ends for exactly one reason: an answer, the caller's signal, or
 * the deadline. The request is aborted for the last two, and the wait ends even
 * if the transport ignores the abort — a request that never answers is the case
 * the deadline exists for, and it may not answer an abort either.
 */
function putWithin(
  transport: AttachmentUploadTransport,
  upload: { ticket: string; mimeType: string; bytes: Blob },
  caller: AbortSignal | undefined,
  timer: UploadTimer,
): Promise<AttachmentUploadReply> {
  return new Promise((resolve, reject) => {
    const request = new AbortController()
    let settled = false
    // Replaced the moment the timer is armed. A timer may spend its whole budget
    // before returning a handle, and then the deadline settles this while there
    // is still nothing to stop; the handle is used below instead.
    let stopTimer = () => {}
    const finish = (settle: () => void) => {
      if (settled) return
      settled = true
      stopTimer()
      caller?.removeEventListener("abort", onCallerAbort)
      settle()
    }
    const stop = (error: NessaAttachmentError) =>
      finish(() => {
        request.abort()
        reject(error)
      })
    const onCallerAbort = () => stop(new NessaAttachmentError("aborted"))
    stopTimer = timer(UPLOAD_DEADLINE_MS, () =>
      stop(new NessaAttachmentError("upload_timeout")),
    )
    // Already out of time before the request was made: nothing to send.
    if (settled) return stopTimer()
    if (caller?.aborted) return onCallerAbort()
    caller?.addEventListener("abort", onCallerAbort, { once: true })
    transport.put({ ...upload, signal: request.signal }).then(
      (reply) => finish(() => resolve(reply)),
      (cause: unknown) =>
        finish(() => reject(new NessaAttachmentError("unreachable", undefined, cause))),
    )
  })
}

export function createAttachmentApi(
  session: RpcRequester,
  transport: AttachmentUploadTransport,
  newId: () => string,
  timer: UploadTimer,
): AttachmentApi {
  return {
    async begin(conversationId, file, options = {}) {
      if (!conversationIdPattern.test(conversationId))
        throw new TypeError("Conversation ID must be a canonical lowercase UUID")
      const requestId = options.requestId ?? newId()
      if (!requestId.trim() || utf8.encode(requestId).byteLength > 256)
        throw new TypeError("Request ID must contain 1-256 UTF-8 bytes")
      if (!validDigest(file.digest))
        throw new TypeError("Digest must be sha256: and 64 lowercase hexadecimal digits")
      if (!validMediaType(file.mimeType))
        throw new TypeError("Media type must be lowercase, without parameters")
      if (!validSize(file.size, MAX_UPLOAD_BYTES))
        throw new TypeError(`Size must be 1-${MAX_UPLOAD_BYTES} bytes`)
      let reply: unknown
      try {
        reply = await session.request(ProductMethod.AttachmentBegin, {
          conversationId,
          requestId,
          digest: file.digest,
          mimeType: file.mimeType,
          size: file.size,
        })
      } catch (cause) {
        // An RPC error is the gateway's answer; anything else is no answer.
        // Told apart by type, and the reason read from its code — never from
        // what either message says.
        if (cause instanceof NessaRpcError)
          throw new NessaAttachmentError(
            "begin_refused",
            undefined,
            cause,
            beginRefusal(cause),
          )
        throw new NessaAttachmentError("unreachable", undefined, cause)
      }
      try {
        return { requestId, ...attachmentBegin(reply, requestId) }
      } catch (cause) {
        throw new NessaAttachmentError("unexpected_response", undefined, cause)
      }
    },
    async upload(ticket, file, options = {}) {
      if (!ticketPattern.test(ticket))
        throw new TypeError("Upload ticket must be 64 lowercase hexadecimal digits")
      if (!validMediaType(file.mimeType))
        throw new TypeError("Media type must be lowercase, without parameters")
      if (!validSize(file.bytes.size, MAX_UPLOAD_BYTES))
        throw new TypeError(`Upload must contain 1-${MAX_UPLOAD_BYTES} bytes`)
      const reply = await putWithin(
        transport,
        { ticket, mimeType: file.mimeType, bytes: file.bytes },
        options.signal,
        timer,
      )
      if (reply.status !== 200)
        throw new NessaAttachmentError(uploadRefusal(reply.body), reply.status)
      try {
        return storedAttachment(reply.body)
      } catch (cause) {
        // The bytes may well be stored, but there is no trustworthy name for
        // them. Beginning again answers `stored` with one.
        throw new NessaAttachmentError("unexpected_response", reply.status, cause)
      }
    },
  }
}
