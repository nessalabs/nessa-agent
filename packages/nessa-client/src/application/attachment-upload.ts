import { NessaRpcError } from "./rpc-error.js"

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
 * The clock an upload's deadline runs on: call `elapsed` once after `ms`, unless
 * the returned function is called first. Composition supplies `setTimeout`;
 * tests supply one they fire by hand, so no test waits.
 */
export type UploadTimer = (ms: number, elapsed: () => void) => () => void

/**
 * How long one upload may go unanswered before this client gives up on it. The
 * gateway allows 120 s for the transfer and then still has to normalize the
 * image, so this sits above both; it exists for the request that never answers
 * at all, which would otherwise leave its caller waiting for good.
 */
export const UPLOAD_DEADLINE_MS = 180_000

/**
 * Why an attachment was not staged.
 *
 * Upload-route refusals, as the gateway names them:
 * - `ticket_invalid`: unknown, expired, or already used.
 * - `size_mismatch`, `digest_mismatch`: not the bytes the ticket was issued for.
 * - `upload_interrupted`: the body stopped arriving.
 * - `attachment_not_kept`: the bytes were good, but the conversation let go of
 *   its files before they were kept. Beginning again stages them.
 * - `upload_timeout`: the transfer outlived the gateway's deadline — or this
 *   client's own, when no answer came at all (then there is no `status`).
 * - `unsupported_image`: not an image format the gateway can read.
 * - `image_too_large`: could not be brought under the selected model's limits.
 * - `image_input_unsupported`: the agent's model takes no images.
 * - `storage_unavailable`, `audit_unavailable`: the gateway could not keep or record it.
 * - `temporarily_unavailable`: too many uploads at once. The only refusal that
 *   does **not** spend the ticket: the same ticket may be tried again shortly.
 *
 * And this client's own: `begin_refused` (the gateway declined to issue a
 * ticket; see `refusal`), `aborted` (the caller's signal), `unreachable` (no
 * answer from either step), `unexpected_response` (an answer this client does
 * not recognise, including a code it has not been taught).
 */
export type AttachmentFailureCode =
  | "ticket_invalid"
  | "size_mismatch"
  | "digest_mismatch"
  | "upload_interrupted"
  | "attachment_not_kept"
  | "upload_timeout"
  | "unsupported_image"
  | "image_too_large"
  | "image_input_unsupported"
  | "storage_unavailable"
  | "audit_unavailable"
  | "temporarily_unavailable"
  | "begin_refused"
  | "aborted"
  | "unreachable"
  | "unexpected_response"

// A closed list on purpose. A code that is not here stays `unexpected_response`
// until somebody decides what it means; it is never passed through as if known.
const uploadRefusals: readonly AttachmentFailureCode[] = [
  "ticket_invalid",
  "size_mismatch",
  "digest_mismatch",
  "upload_interrupted",
  "attachment_not_kept",
  "upload_timeout",
  "unsupported_image",
  "image_too_large",
  "image_input_unsupported",
  "storage_unavailable",
  "audit_unavailable",
  "temporarily_unavailable",
]

/** A refusal code the upload route is known to give, or `unexpected_response`. */
export function uploadRefusal(body: unknown): AttachmentFailureCode {
  const code =
    body && typeof body === "object" && !Array.isArray(body)
      ? (body as { code?: unknown }).code
      : undefined
  return (
    uploadRefusals.find((known) => known === code) ?? ("unexpected_response" as const)
  )
}

/**
 * Why the gateway declined `attachment.begin`, as it named it. `unexpected` is a
 * code this client has not been taught. `attachment_capacity` and
 * `temporarily_unavailable` pass with time — capacity frees as tickets are used
 * or expire — so asking again later may work. The upload route says
 * `storage_unavailable` where the socket says `attachment_storage_unavailable`;
 * both are here.
 */
export type AttachmentBeginRefusal =
  | "invalid_request"
  | "conversation_not_found"
  | "attachment_capacity"
  | "attachment_storage_unavailable"
  | "storage_unavailable"
  | "audit_unavailable"
  | "temporarily_unavailable"
  | "agent_not_configured"
  | "unexpected"

const beginRefusals: readonly AttachmentBeginRefusal[] = [
  "invalid_request",
  "conversation_not_found",
  "attachment_capacity",
  "attachment_storage_unavailable",
  "storage_unavailable",
  "audit_unavailable",
  "temporarily_unavailable",
  "agent_not_configured",
]

/** The gateway's reason for refusing `begin`, read from the RPC error's code and nothing else. */
export function beginRefusal(cause: NessaRpcError): AttachmentBeginRefusal {
  return beginRefusals.find((known) => known === cause.code) ?? "unexpected"
}

/**
 * An attachment that was not staged, with a code to branch on.
 *
 * Nothing here is replayed for you. After `temporarily_unavailable` the ticket
 * is still good and the same upload may be tried again; after every other upload
 * failure the ticket is spent or of unknown state, so begin again — bytes that
 * did arrive answer `stored`, and bytes that did not get a fresh ticket. The
 * message and cause never contain the ticket.
 */
export class NessaAttachmentError extends Error {
  constructor(
    /** Which refusal or fault this was. */
    readonly code: AttachmentFailureCode,
    /** HTTP status of the upload route's answer, when there was one. */
    readonly status?: number,
    cause?: unknown,
    /** The gateway's own reason, when `code` is `begin_refused`. */
    readonly refusal?: AttachmentBeginRefusal,
  ) {
    super(`Attachment was not staged (${refusal ? `${code}: ${refusal}` : code})`, {
      cause,
    })
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
