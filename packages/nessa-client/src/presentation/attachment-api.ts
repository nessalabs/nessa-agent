import {
  beginRefusal,
  NessaAttachmentError,
  UPLOAD_DEADLINE_MS,
  uploadRefusal,
  type AttachmentUploadTransport,
} from "../application/attachment-upload.js"
import { requestWithin, type RequestTimer } from "../application/gateway-http.js"
import {
  HELD_FILE_BUSY_RETRY_DELAYS_MS,
  HELD_FILE_RANGE_BYTES,
  heldFileRefusal,
  NessaHeldFileError,
} from "../application/held-file-read.js"
import { NessaRpcError } from "../application/rpc-error.js"
import type { RpcRequester } from "../application/session-port.js"
import { ProductMethod } from "../generated/product.js"
import {
  artifactReadRange,
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
       * given, because the gateway normalizes images. `asImageAttachment`
       * narrows image references; general files add their original name in
       * `conversation.prepare`'s `uploads` option.
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
 * A file the conversation holds — an image uploaded into it, or an artifact
 * its agent published — is read back with `fetch`, and handed over only once
 * its size and SHA-256 are the ones its reference names.
 *
 * Every failure of `begin` and `upload` is a {@link NessaAttachmentError} with a
 * code, and nothing is retried for you; `fetch` fails with a
 * {@link NessaHeldFileError}.
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
  /**
   * Read back a whole file the conversation holds, verified.
   *
   * Asks `artifact.read` for one range of {@link HELD_FILE_RANGE_BYTES} after
   * another, from the start, each only once the last was answered. Every
   * answer is held to the range asked: its offset, canonical base64, and
   * exactly the bytes left up to the range's size — fewer only where the file
   * ends. The assembled bytes are returned only when their length and SHA-256
   * are the ones `file` names; nothing unverified is ever handed back, not
   * even in part. A range the gateway answers `temporarily_unavailable` is
   * asked again after each of {@link HELD_FILE_BUSY_RETRY_DELAYS_MS} on this
   * client's timer; every other failure ends the fetch at once.
   * @param conversationId - Canonical lowercase UUID of the conversation that holds it.
   * @param file - Exactly as the conversation's view names it: an
   * `ImageAttachment` of a message, or an artifact's `digest`, `mimeType`
   * and `size`. The same bytes held as another media type are another file.
   * @param options - `signal` abandons the fetch between or during ranges,
   * which then fails as `aborted`; a range already asked for is still answered
   * to the session, and dropped.
   * @returns The verified bytes, as a `Blob` of `file.mimeType`.
   * @throws TypeError for arguments the gateway would refuse, before anything
   * is sent; otherwise {@link NessaHeldFileError} — see
   * `HeldFileFailureCode` for each code.
   */
  fetch: (
    conversationId: string,
    file: AttachmentDescription,
    options?: { signal?: AbortSignal },
  ) => Promise<Blob>
}

const ticketPattern = /^[0-9a-f]{64}$/
const utf8 = new TextEncoder()

function hex(digest: ArrayBuffer): string {
  return Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("")
}

/**
 * Settles with `work`, or rejects as `aborted` the moment `signal` aborts,
 * whichever is first. What `work` was doing is not stopped, only no longer
 * waited for: a request on the session has no way to be withdrawn.
 */
function unlessAborted<T>(work: Promise<T>, signal: AbortSignal | undefined): Promise<T> {
  if (!signal) return work
  return new Promise((resolve, reject) => {
    const onAbort = () => reject(new NessaHeldFileError("aborted"))
    if (signal.aborted) return onAbort()
    signal.addEventListener("abort", onAbort, { once: true })
    work.then(
      (value) => {
        signal.removeEventListener("abort", onAbort)
        resolve(value)
      },
      (cause: unknown) => {
        signal.removeEventListener("abort", onAbort)
        reject(cause)
      },
    )
  })
}

/** Resolves after `ms` on `timer`, or rejects as `aborted` when `signal` aborts first. */
function waitOn(timer: RequestTimer, ms: number, signal: AbortSignal | undefined) {
  return new Promise<void>((resolve, reject) => {
    const onAbort = () => {
      stop()
      reject(new NessaHeldFileError("aborted"))
    }
    const stop = timer(ms, () => {
      signal?.removeEventListener("abort", onAbort)
      resolve()
    })
    if (signal?.aborted) return onAbort()
    signal?.addEventListener("abort", onAbort, { once: true })
  })
}

export function createAttachmentApi(
  session: RpcRequester,
  transport: AttachmentUploadTransport,
  newId: () => string,
  timer: RequestTimer,
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
      const reply = await requestWithin(
        (signal) =>
          transport.put({ ticket, mimeType: file.mimeType, bytes: file.bytes, signal }),
        options.signal,
        timer,
        UPLOAD_DEADLINE_MS,
        {
          aborted: () => new NessaAttachmentError("aborted"),
          timeout: () => new NessaAttachmentError("upload_timeout"),
          unreachable: (cause) =>
            new NessaAttachmentError("unreachable", undefined, cause),
        },
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
    async fetch(conversationId, file, options = {}) {
      if (!conversationIdPattern.test(conversationId))
        throw new TypeError("Conversation ID must be a canonical lowercase UUID")
      if (!validDigest(file.digest))
        throw new TypeError("Digest must be sha256: and 64 lowercase hexadecimal digits")
      if (!validMediaType(file.mimeType))
        throw new TypeError("Media type must be lowercase, without parameters")
      if (!validSize(file.size, MAX_UPLOAD_BYTES))
        throw new TypeError(`Size must be 1-${MAX_UPLOAD_BYTES} bytes`)
      // Read once: what was checked is what is asked for and verified against.
      const { digest, mimeType, size } = file
      const { signal } = options
      if (signal?.aborted) throw new NessaHeldFileError("aborted")
      const bytes = new Uint8Array(size)
      let offset = 0
      while (offset < size) {
        // Exactly this many come back: a whole range, or what is left of the file.
        const length = Math.min(HELD_FILE_RANGE_BYTES, size - offset)
        let reply: unknown
        for (let attempt = 0; ; attempt++) {
          try {
            reply = await unlessAborted(
              session.request(ProductMethod.ArtifactRead, {
                conversationId,
                digest,
                mimeType,
                size,
                offset,
                maxBytes: HELD_FILE_RANGE_BYTES,
              }),
              signal,
            )
            break
          } catch (cause) {
            if (cause instanceof NessaHeldFileError) throw cause
            const code = heldFileRefusal(cause)
            const delay = HELD_FILE_BUSY_RETRY_DELAYS_MS[attempt]
            if (code !== "temporarily_unavailable" || delay === undefined)
              throw new NessaHeldFileError(code, cause)
            await waitOn(timer, delay, signal)
          }
        }
        let range: Uint8Array
        try {
          range = artifactReadRange(reply, offset, length)
        } catch (cause) {
          throw new NessaHeldFileError("unexpected_response", cause)
        }
        bytes.set(range, offset)
        offset += length
      }
      // The only check that holds for what arrived: every range was the one
      // asked for, but only the whole can say they are the file's bytes.
      const actual = `sha256:${hex(await crypto.subtle.digest("SHA-256", bytes))}`
      if (signal?.aborted) throw new NessaHeldFileError("aborted")
      if (actual !== digest) throw new NessaHeldFileError("integrity")
      return new Blob([bytes], { type: mimeType })
    },
  }
}
