import type { ImageAttachment } from "../generated/product.js"

/**
 * Bounds the protocol schema puts on these values. They are the contract's, not
 * any model's: what a particular model takes is the gateway's knowledge, applied
 * when it stores an upload, and is written nowhere in this package.
 */
/** `ImageAttachment.size` maximum. */
export const MAX_IMAGE_ATTACHMENT_BYTES = 5_242_880
/** `attachments` maxItems on send, steer, messages, and pending input. */
export const MAX_MESSAGE_IMAGES = 10
/** Total image bytes one message may refer to: base64 of it must fit one 16 MiB agent frame. */
export const MAX_MESSAGE_IMAGE_BYTES = 10 * 1024 * 1024
/** `AttachmentBeginParams.size` maximum: what the upload path takes, whatever it becomes. */
export const MAX_UPLOAD_BYTES = 67_108_864

/**
 * The four encodings a message's image may be in — `ImageAttachment.mimeType`.
 * Exported once from this package. Storage is wider than this: see
 * {@link StoredAttachment}.
 */
export const IMAGE_ATTACHMENT_TYPES = [
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
] as const

const digestPattern = /^sha256:[0-9a-f]{64}$/
const ticketPattern = /^[0-9a-f]{64}$/
const mediaTypePattern = /^[a-z0-9][a-z0-9!#$&^_.+-]*\/[a-z0-9][a-z0-9!#$&^_.+-]*$/

export function validDigest(value: unknown): value is string {
  return typeof value === "string" && digestPattern.test(value)
}
export function validMediaType(value: unknown): value is string {
  return typeof value === "string" && value.length <= 127 && mediaTypePattern.test(value)
}
export function validSize(value: unknown, max: number): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 1 && (value as number) <= max
}

/**
 * What a conversation holds for an upload: any media type the upload path
 * takes. Storage is media-agnostic; whether a message may refer to it is a
 * separate question, answered by {@link asImageAttachment}.
 */
export type StoredAttachment = {
  /** `sha256:` and 64 lowercase hexadecimal digits, over the stored bytes. */
  digest: string
  /** Lowercase media type without parameters. */
  mimeType: string
  /** Stored length in bytes. */
  size: number
}

function exactReference(item: unknown): Record<string, unknown> | string {
  if (!item || typeof item !== "object" || Array.isArray(item))
    return "an attachment must be an object"
  if (Object.keys(item).some((key) => !["digest", "mimeType", "size"].includes(key)))
    return "an attachment has unknown fields"
  return item as Record<string, unknown>
}

/** Why a value is not a stored reference of any media type, or undefined when it is. */
export function storedAttachmentProblem(item: unknown): string | undefined {
  const reference = exactReference(item)
  if (typeof reference === "string") return reference
  if (!validDigest(reference.digest))
    return "an attachment digest must be sha256: and 64 lowercase hexadecimal digits"
  if (!validMediaType(reference.mimeType))
    return "an attachment media type must be lowercase, without parameters"
  if (!validSize(reference.size, MAX_UPLOAD_BYTES))
    return `an attachment must contain 1-${MAX_UPLOAD_BYTES} bytes`
  return undefined
}

/** Why a value is not one image a message may refer to, or undefined when it is. */
export function imageAttachmentProblem(item: unknown): string | undefined {
  const reference = exactReference(item)
  if (typeof reference === "string") return reference
  if (!validDigest(reference.digest))
    return "an attachment digest must be sha256: and 64 lowercase hexadecimal digits"
  if (!(IMAGE_ATTACHMENT_TYPES as readonly unknown[]).includes(reference.mimeType))
    return "an attachment must be a PNG, JPEG, GIF, or WebP image"
  if (!validSize(reference.size, MAX_IMAGE_ATTACHMENT_BYTES))
    return `an image must contain 1-${MAX_IMAGE_ATTACHMENT_BYTES} bytes`
  return undefined
}

/**
 * The explicit step from "the conversation holds this" to "a message may name
 * it": the same reference as an {@link ImageAttachment} when it is one of the
 * four image encodings within the protocol's image bound, otherwise undefined.
 * A stored PDF is a valid upload and no image.
 */
export function asImageAttachment(stored: StoredAttachment): ImageAttachment | undefined {
  const image = { digest: stored.digest, mimeType: stored.mimeType, size: stored.size }
  return imageAttachmentProblem(image) === undefined
    ? (image as ImageAttachment)
    : undefined
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
 * from the ones sent — the gateway normalizes images — and a malformed one is no
 * reference at all.
 */
export function storedAttachment(value: unknown): StoredAttachment {
  const problem = storedAttachmentProblem(value)
  if (problem) throw new Error(`Invalid stored attachment: ${problem}`)
  const { digest, mimeType, size } = value as StoredAttachment
  return { digest, mimeType, size }
}

/** What `attachment.begin` answered, with each state carrying only what it means. */
export type AttachmentBeginReply =
  | { state: "stored"; stored: StoredAttachment }
  | { state: "upload_required"; ticket: string; expiresAtMs: number }

const beginKeys = [
  "requestId",
  "state",
  "ticket",
  "expiresAtMs",
  "digest",
  "mimeType",
  "size",
]

/**
 * The answer to `attachment.begin`, with its fields checked against each other.
 *
 * Every field is always present; "not here" is written as null, and a reply
 * that leaves one out is a different contract and is refused. The stored
 * reference is non-null exactly when the state is `stored`, and the ticket and
 * its expiry exactly when it is `upload_required`. Each of the other
 * combinations — stored with a ticket, stored with no reference, a ticket owed
 * and absent, a ticket beside a reference — is built from individually valid
 * fields and describes nothing that can have happened, so it is refused rather
 * than interpreted. Messages here never include the ticket.
 */
export function attachmentBegin(value: unknown, requestId: string): AttachmentBeginReply {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("Invalid attachment response")
  const item = value as Record<string, unknown>
  if (Object.keys(item).some((key) => !beginKeys.includes(key)))
    throw new Error("Attachment response has unknown fields")
  if (beginKeys.some((key) => item[key] === undefined))
    throw new Error("Attachment response is missing fields")
  if (item.requestId !== requestId)
    throw new Error("Attachment response belongs to another action")
  const noTicket = item.ticket === null && item.expiresAtMs === null
  const noReference = item.digest === null && item.mimeType === null && item.size === null
  if (item.state === "stored") {
    if (!noTicket) throw new Error("Stored attachment response carries an upload ticket")
    return {
      state: "stored",
      stored: storedAttachment({
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
