import {
  NessaAttachmentError,
  uploadRefusal,
  type AttachmentUploadTransport,
} from "../application/attachment-upload.js"
import { NessaRpcError } from "../application/rpc-error.js"
import type { RpcRequester } from "../application/session-port.js"
import { ProductMethod, type ImageAttachment } from "../generated/product.js"
import {
  attachmentBegin,
  MAX_UPLOAD_BYTES,
  storedAttachment,
  validDigest,
  validMediaType,
  validSize,
} from "../protocol/attachment-validate.js"
import type { ConversationActionOptions } from "./conversation-api.js"

/** The bytes an upload is for: what they hash to, what they are, how many there are. */
export type AttachmentDescription = {
  /** `sha256:` and 64 lowercase hexadecimal digits, over exactly the bytes to upload. */
  digest: string
  /** Lowercase media type without parameters, at most 127 characters. */
  mimeType: string
  /** Exact length in bytes, 1 to 20 MiB. */
  size: number
}

/** The conversation already holds these bytes, or here is the ticket to upload them once. */
export type AttachmentBeginning =
  | {
      requestId: string
      state: "stored"
      /**
       * What the conversation holds them as: the reference to send. It may
       * differ from the description given, because the gateway normalizes images.
       */
      image: ImageAttachment
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
 * Put a file where a message can refer to it.
 *
 * Bytes never ride in a conversation command. Begin over the socket, which knows
 * who is asking; upload the original bytes over HTTP with the ticket that answer
 * carried; then name them in `conversation.send` by the reference the gateway
 * gives back. That reference is the gateway's, not yours: it converts, scales,
 * and compresses images to what the selected model takes, so the stored digest,
 * media type, and size may all differ from what was uploaded. Never send a
 * digest you computed.
 *
 * Every failure is a {@link NessaAttachmentError} with a code. Nothing is
 * retried for you; to try again, begin again.
 */
export type AttachmentApi = {
  /**
   * Ask to upload into a conversation that already exists on the gateway
   * (`conversation.create` is idempotent; call it first).
   * @param conversationId - Canonical lowercase UUID of that conversation.
   * @param file - Digest, media type, and size of exactly the bytes to upload.
   * Any media type up to 20 MiB; what becomes of it is the gateway's decision.
   * @param options - Optional caller-managed action identity.
   * @returns `stored` with the reference to send when nothing further is
   * needed, otherwise a ticket.
   * @throws TypeError for arguments the gateway would refuse;
   * NessaAttachmentError `begin_refused` when it did refuse, `unreachable` when
   * no answer arrived, `unexpected_response` for an answer that contradicts itself.
   */
  begin: (
    conversationId: string,
    file: AttachmentDescription,
    options?: ConversationActionOptions,
  ) => Promise<AttachmentBeginning>
  /**
   * Send the bytes a ticket was issued for.
   * @param ticket - From `begin`. Spent by this call whether or not it succeeds.
   * @param file - The same media type given to `begin`, and the bytes themselves.
   * @param options - `signal` abandons the request.
   * @returns The reference the gateway stored them as — the only thing to pass
   * to `conversation.send`.
   * @throws NessaAttachmentError `ticket_invalid` (unknown, expired, or used),
   * `size_mismatch`, `digest_mismatch`, `unsupported_image` (not an image format
   * the gateway can read), `image_too_large` (could not be brought under the
   * model's limits), `storage_unavailable`, `audit_unavailable`, `unreachable`
   * when no answer arrived, or `unexpected_response` — including a success whose
   * reference is malformed.
   */
  upload: (
    ticket: string,
    file: { mimeType: string; bytes: Blob },
    options?: { signal?: AbortSignal },
  ) => Promise<ImageAttachment>
}

const conversationIdPattern =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const ticketPattern = /^[0-9a-f]{64}$/
const utf8 = new TextEncoder()

export function createAttachmentApi(
  session: RpcRequester,
  transport: AttachmentUploadTransport,
  newId: () => string,
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
        // Told apart by type, never by what the message says.
        throw new NessaAttachmentError(
          cause instanceof NessaRpcError ? "begin_refused" : "unreachable",
          undefined,
          cause,
        )
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
      let reply
      try {
        reply = await transport.put({
          ticket,
          mimeType: file.mimeType,
          bytes: file.bytes,
          signal: options.signal,
        })
      } catch (cause) {
        throw new NessaAttachmentError("unreachable", undefined, cause)
      }
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
