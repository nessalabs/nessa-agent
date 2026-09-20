import type { MessageContent } from "./content"

/**
 * Why an upload did not finish, as something to branch on.
 *
 * `unreadable`: the bytes could not be read or hashed in this window.
 * `unsupported-image`: the gateway could not read this image format.
 * `too-large`: the gateway could not bring it under the selected model's limits.
 * `unavailable`: the gateway or its storage could not be reached; trying again may work.
 * `rejected`: the gateway answered and refused these bytes for another reason.
 */
export type UploadFailure =
  "unreadable" | "unsupported-image" | "too-large" | "unavailable" | "rejected"

/** The four encodings the gateway stores an image as, and so the four a message names. */
export const STORED_IMAGE_TYPES = [
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
] as const
export type StoredImageType = (typeof STORED_IMAGE_TYPES)[number]

/**
 * An image the gateway holds, named the way a message names it. Never bytes.
 *
 * It is always the gateway's own answer. The gateway converts, scales, and
 * compresses what is uploaded to what the selected model takes, so this digest,
 * encoding, and size are of the stored image — not of the file that was
 * attached, and not anything this window could have computed.
 */
export type ImageReference = { digest: string; mimeType: StoredImageType; size: number }

/**
 * Where a file's bytes are, from this window's point of view.
 *
 * A file that is not an image stays `not-started`: nothing uploads it, and
 * sending refuses it with a reason. Only `stored` carries a reference, and it
 * carries the whole one, so there is no way to write down an upload that
 * finished without saying what it finished as.
 */
export type UploadState =
  | { status: "not-started" }
  | { status: "uploading" }
  | { status: "stored"; image: ImageReference }
  | { status: "failed"; reason: UploadFailure }

/**
 * A file attached in this window. `name`, `mimeType`, `size`, and `previewUrl`
 * describe the original file, which is what is previewed and what is uploaded.
 * What a message will refer to is `upload.image` and may be none of those.
 * Plain data: the bytes themselves stay with the panel's resource store.
 */
export type FileAttachment = {
  type: "file"
  id: string
  name: string
  mimeType: string
  size: number
  previewUrl: string
  upload: UploadState
}

/**
 * An image known only by reference: a turn read back from the gateway, which
 * this window never held the bytes for. It has nothing to preview.
 */
export type ImageReferencePart = { type: "image-reference" } & ImageReference

/**
 * Preview budgets: what one window will hold. They bound drafts, not messages.
 * The per-file figure is also what the upload path takes, and it is the only
 * byte limit this window puts on a single file: how heavy an image may be for a
 * model is the gateway's rule, applied when it stores one.
 */
export const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024
export const MAX_DRAFT_ATTACHMENT_BYTES = 50 * 1024 * 1024
export const MAX_DRAFT_ATTACHMENTS = 20

/** Message rules, counted over stored references. The gateway enforces the same. */
export const MAX_SEND_IMAGES = 10
// Ten, not twenty: the agent is handed base64, a third larger, in one 16 MiB frame.
export const MAX_SEND_TOTAL_IMAGE_BYTES = 10 * 1024 * 1024

const digestPattern = /^sha256:[0-9a-f]{64}$/

/** Enforce per-file and aggregate draft budgets before changing stored content. */
export function validDraftAttachments(content: MessageContent): boolean {
  const files = content.filter((part) => part.type === "file")
  return (
    files.length <= MAX_DRAFT_ATTACHMENTS &&
    new Set(files.map((file) => file.id)).size === files.length &&
    files.every(
      (file) =>
        Number.isSafeInteger(file.size) &&
        file.size >= 0 &&
        file.size <= MAX_ATTACHMENT_BYTES,
    ) &&
    files.reduce((bytes, file) => bytes + file.size, 0) <= MAX_DRAFT_ATTACHMENT_BYTES
  )
}

/**
 * Whether a file is uploaded as an image. Any `image/*` is: which formats can
 * actually be read is the gateway's to say, and it says so per upload.
 */
export function isImageFile(mimeType: string): boolean {
  return mimeType.startsWith("image/")
}

/** Whether a value is a reference a message could name. Its size is the gateway's business. */
export function validImageReference(value: {
  digest: unknown
  mimeType: unknown
  size: unknown
}): value is ImageReference {
  return (
    typeof value.digest === "string" &&
    digestPattern.test(value.digest) &&
    (STORED_IMAGE_TYPES as readonly unknown[]).includes(value.mimeType) &&
    Number.isSafeInteger(value.size) &&
    (value.size as number) >= 1
  )
}

/**
 * Why a message's files cannot go, in the order somebody would want to hear it:
 * what can never go, then what failed, then what is merely not ready, then size.
 */
export type ImageRefusal =
  | { kind: "unsupported-file"; name: string }
  | { kind: "upload-failed"; name: string }
  | { kind: "upload-in-flight" }
  | { kind: "too-many-images" }
  | { kind: "images-too-large" }

/**
 * The images this content would send, or the one reason it cannot.
 *
 * All or nothing: a message that silently left one file behind would be a
 * different message from the one on screen. What goes is each file's stored
 * reference — never anything derived from the file itself — and a stored
 * reference that is not one names nothing the gateway holds, so it counts as an
 * upload that failed.
 */
export function messageImages(
  content: MessageContent,
): { ok: true; images: ImageReference[] } | { ok: false; refusal: ImageRefusal } {
  const files = content.filter((part) => part.type === "file")
  const unsupported = files.find((file) => !isImageFile(file.mimeType))
  if (unsupported)
    return { ok: false, refusal: { kind: "unsupported-file", name: unsupported.name } }
  const failed = files.find(
    (file) =>
      file.upload.status === "failed" ||
      (file.upload.status === "stored" && !validImageReference(file.upload.image)),
  )
  if (failed) return { ok: false, refusal: { kind: "upload-failed", name: failed.name } }
  if (files.some((file) => file.upload.status !== "stored"))
    return { ok: false, refusal: { kind: "upload-in-flight" } }
  const images = content.flatMap((part): ImageReference[] => {
    const image =
      part.type === "image-reference"
        ? part
        : part.type === "file" && part.upload.status === "stored"
          ? part.upload.image
          : undefined
    return image
      ? [{ digest: image.digest, mimeType: image.mimeType, size: image.size }]
      : []
  })
  if (images.length > MAX_SEND_IMAGES)
    return { ok: false, refusal: { kind: "too-many-images" } }
  if (images.reduce((bytes, image) => bytes + image.size, 0) > MAX_SEND_TOTAL_IMAGE_BYTES)
    return { ok: false, refusal: { kind: "images-too-large" } }
  return { ok: true, images }
}

/** Bytes as a person reads them: "812 KB", "3.4 MB". */
export function humanSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

/** What stands in for an image this window cannot show: "PNG image, 812 KB". */
export function imageReferenceLabel(mimeType: StoredImageType, size: number): string {
  const kind = {
    "image/png": "PNG",
    "image/jpeg": "JPEG",
    "image/gif": "GIF",
    "image/webp": "WebP",
  }
  return `${kind[mimeType]} image, ${humanSize(size)}`
}

/** What to call a message where only text fits — a tab title, a queue row. */
export function messageLabel(text: string, imageCount: number, firstImageName?: string) {
  const trimmed = text.trim()
  if (trimmed) return trimmed
  if (imageCount === 0) return ""
  if (imageCount === 1) return firstImageName?.trim() || "1 image"
  return `${imageCount} images`
}
