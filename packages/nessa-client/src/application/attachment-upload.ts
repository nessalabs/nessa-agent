/** One upload, as the route needs it. The ticket is a secret: never log it. */
export type AttachmentUploadRequest = {
  /** Single-use ticket from `attachment.begin`. */
  ticket: string
  /** The media type declared when the ticket was issued. */
  mimeType: string
  /** Exactly the bytes whose digest and size the ticket was issued for. */
  bytes: Blob
  /** Abandons the request. The ticket may or may not have been spent. */
  signal?: AbortSignal
}

/**
 * What the upload route answered: its status, and its body when that was JSON.
 * Unvalidated on purpose — what a body means is decided by the caller of this
 * port, in one place, not by whichever adapter happened to fetch it.
 */
export type AttachmentUploadReply = { status: number; body: unknown }

/**
 * The upload route, as this package needs it. HTTP is on the other side.
 *
 * The socket cannot carry bytes — one message is capped at 64 KiB — so they
 * travel on their own request. This is the seam for it: composition supplies
 * the `fetch`-backed adapter, tests supply one that can refuse, stall, or fail.
 * An adapter rejects only when no answer arrived at all.
 */
export interface AttachmentUploadTransport {
  put(upload: AttachmentUploadRequest): Promise<AttachmentUploadReply>
}

/**
 * Why an attachment was not staged.
 *
 * All but the last three are the upload route's own refusals: the ticket was
 * unknown, expired, or used; the bytes were not the size or digest it was issued
 * for; the gateway could not read this image format; the image could not be
 * brought under the selected model's limits; storage or the audit record was
 * unavailable. `begin_refused` is the gateway declining to issue a ticket,
 * `unreachable` is no answer from either step, and `unexpected_response` is an
 * answer this client does not recognise.
 */
export type AttachmentFailureCode =
  | "ticket_invalid"
  | "size_mismatch"
  | "digest_mismatch"
  | "unsupported_image"
  | "image_too_large"
  | "storage_unavailable"
  | "audit_unavailable"
  | "begin_refused"
  | "unreachable"
  | "unexpected_response"

const uploadRefusals: readonly string[] = [
  "ticket_invalid",
  "size_mismatch",
  "digest_mismatch",
  "unsupported_image",
  "image_too_large",
  "storage_unavailable",
  "audit_unavailable",
]

/** A refusal code the upload route is known to give, or `unexpected_response`. */
export function uploadRefusal(body: unknown): AttachmentFailureCode {
  const code =
    body && typeof body === "object" && !Array.isArray(body)
      ? (body as { code?: unknown }).code
      : undefined
  return typeof code === "string" && uploadRefusals.includes(code)
    ? (code as AttachmentFailureCode)
    : "unexpected_response"
}

/**
 * An attachment that was not staged, with a code to branch on.
 *
 * Nothing here is replayed for you. A ticket is single-use, so after any upload
 * failure begin again: bytes that did arrive answer `stored`, and bytes that did
 * not get a fresh ticket. The message and cause never contain the ticket.
 */
export class NessaAttachmentError extends Error {
  constructor(
    /** Which refusal or fault this was. */
    readonly code: AttachmentFailureCode,
    /** HTTP status of the upload route's answer, when there was one. */
    readonly status?: number,
    cause?: unknown,
  ) {
    super(`Attachment was not staged (${code})`, { cause })
    this.name = "NessaAttachmentError"
  }
}

/**
 * Where uploads go for a session: the gateway's own origin, over HTTP.
 *
 * Same host and port as the WebSocket, `ws` to `http` and `wss` to `https`, and
 * no path — `/session` and `/browser/session` are socket routes. Pure, so the
 * rule is tested without a connection.
 */
export function attachmentUploadUrl(sessionUrl: string): string {
  const url = new URL(sessionUrl)
  if (url.protocol !== "ws:" && url.protocol !== "wss:")
    throw new TypeError("Session URL must use ws: or wss:")
  return `${url.protocol === "wss:" ? "https:" : "http:"}//${url.host}/attachments`
}
