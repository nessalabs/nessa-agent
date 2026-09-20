import type { ImageAttachment } from "../generated/product.js"

/**
 * What one message may carry. These are rules of the message, not of any model:
 * how heavy a single image may be is the gateway's to decide when it stores one,
 * so nothing here bounds an image except the total it counts towards.
 */
export const MAX_MESSAGE_IMAGES = 10
// Ten, not twenty: the agent receives base64, a third larger, in one 16 MiB frame.
export const MAX_MESSAGE_IMAGE_BYTES = 10 * 1024 * 1024
/** What the upload path takes, whatever a message may later refer to. */
export const MAX_UPLOAD_BYTES = 20 * 1024 * 1024

const digestPattern = /^sha256:[0-9a-f]{64}$/
const ticketPattern = /^[0-9a-f]{64}$/
const mediaTypePattern = /^[a-z0-9][a-z0-9!#$&^_.+-]*\/[a-z0-9][a-z0-9!#$&^_.+-]*$/
const imageTypes = ["image/png", "image/jpeg", "image/gif", "image/webp"]

export function validDigest(value: unknown): value is string {
  return typeof value === "string" && digestPattern.test(value)
}
export function validMediaType(value: unknown): value is string {
  return typeof value === "string" && value.length <= 127 && mediaTypePattern.test(value)
}
export function validSize(value: unknown, max: number): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 1 && (value as number) <= max
}

/** Why a value is not one stored image reference, or undefined when it is. */
export function imageAttachmentProblem(item: unknown): string | undefined {
  if (!item || typeof item !== "object" || Array.isArray(item))
    return "an attachment must be an object"
  if (Object.keys(item).some((key) => !["digest", "mimeType", "size"].includes(key)))
    return "an attachment has unknown fields"
  const { digest, mimeType, size } = item as Record<string, unknown>
  if (!validDigest(digest))
    return "an attachment digest must be sha256: and 64 lowercase hexadecimal digits"
  if (typeof mimeType !== "string" || !imageTypes.includes(mimeType))
    return "an attachment must be a PNG, JPEG, GIF, or WebP image"
  if (!validSize(size, MAX_MESSAGE_IMAGE_BYTES))
    return `an image must contain 1-${MAX_MESSAGE_IMAGE_BYTES} bytes`
  return undefined
}

/**
 * Why a list is not a message's images, or undefined when it is.
 *
 * One rule for both directions: the client refuses to send what the gateway
 * would refuse, and refuses to believe a reply the gateway could not have
 * built. Callers choose the error type; a bad argument is not a bad reply.
 */
export function imageAttachmentsProblem(value: unknown): string | undefined {
  if (!Array.isArray(value)) return "attachments must be a list"
  if (value.length > MAX_MESSAGE_IMAGES)
    return `a message carries at most ${MAX_MESSAGE_IMAGES} images`
  let total = 0
  for (const item of value as unknown[]) {
    const problem = imageAttachmentProblem(item)
    if (problem) return problem
    total += (item as ImageAttachment).size
  }
  if (total > MAX_MESSAGE_IMAGE_BYTES)
    return `a message carries at most ${MAX_MESSAGE_IMAGE_BYTES} bytes of images`
  return undefined
}

/** A reply's images, or an error: the gateway never sends a list it would not accept. */
export function imageAttachments(value: unknown, where: string): ImageAttachment[] {
  const problem = imageAttachmentsProblem(value)
  if (problem) throw new Error(`Invalid conversation ${where}: ${problem}`)
  return value as ImageAttachment[]
}

/**
 * The reference the gateway stored an upload as. It may name different bytes
 * from the ones sent — the gateway normalizes images — so it is the only thing
 * a message may refer to, and a malformed one is no reference at all.
 */
export function storedAttachment(value: unknown): ImageAttachment {
  const problem = imageAttachmentProblem(value)
  if (problem) throw new Error(`Invalid stored attachment: ${problem}`)
  const { digest, mimeType, size } = value as ImageAttachment
  return { digest, mimeType, size }
}

/** What `attachment.begin` answered, with each state carrying only what it means. */
export type AttachmentBeginReply =
  | { state: "stored"; image: ImageAttachment }
  | { state: "upload_required"; ticket: string; expiresAtMs: number }

/**
 * The answer to `attachment.begin`, with its fields checked against each other.
 *
 * The stored reference is present exactly when the state is `stored`, and the
 * ticket and its expiry exactly when it is `upload_required`. Each of the other
 * combinations — stored with a ticket, stored with no reference, a ticket owed
 * and absent, a ticket beside a reference — is built from individually valid
 * fields and describes nothing that can have happened, so it is refused rather
 * than interpreted. Messages here never include the ticket.
 */
export function attachmentBegin(value: unknown, requestId: string): AttachmentBeginReply {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("Invalid attachment response")
  const item = value as Record<string, unknown>
  const allowed = [
    "requestId",
    "state",
    "ticket",
    "expiresAtMs",
    "digest",
    "mimeType",
    "size",
  ]
  if (Object.keys(item).some((key) => !allowed.includes(key)))
    throw new Error("Attachment response has unknown fields")
  if (item.requestId !== requestId)
    throw new Error("Attachment response belongs to another action")
  // The contract writes "not here" as null. An absent field says the same and
  // is read the same; what matters is which fields carry a value.
  const absent = (key: string) => item[key] === null || item[key] === undefined
  const noTicket = absent("ticket") && absent("expiresAtMs")
  const noReference = absent("digest") && absent("mimeType") && absent("size")
  if (item.state === "stored") {
    if (!noTicket) throw new Error("Stored attachment response carries an upload ticket")
    return {
      state: "stored",
      image: storedAttachment({
        digest: item.digest,
        mimeType: item.mimeType,
        size: item.size,
      }),
    }
  }
  if (item.state === "upload_required") {
    if (!noReference)
      throw new Error("Attachment response owes an upload and names stored bytes")
    if (typeof item.ticket !== "string" || !ticketPattern.test(item.ticket))
      throw new Error("Attachment response has no usable upload ticket")
    if (!Number.isSafeInteger(item.expiresAtMs) || (item.expiresAtMs as number) < 0)
      throw new Error("Attachment response has no ticket expiry")
    return {
      state: "upload_required",
      ticket: item.ticket,
      expiresAtMs: item.expiresAtMs as number,
    }
  }
  throw new Error("Invalid attachment state")
}
