/**
 * An image or any other file put on a message before it is sent — pasted
 * into a composer or picked with its Attach button — by what the window shows
 * of it and where its bytes are. `kind` says which of the two it is, decided
 * once when it is put on (`draftKind`): an image goes to the model as an
 * image, any other file is placed where the agent works.
 *
 * `url` is an object URL over the bytes the paste carried. It is both the
 * tile's picture and, at send, what the upload reads back, so the state holds
 * strings and never a `Blob`. It lives exactly as long as the workspace names
 * it, in a composer or on a message not yet in its conversation
 * (`releasedImageUrls`, which the store's effects revoke, lets it go after).
 */
export interface DraftImage {
  readonly id: string
  readonly kind: "image" | "file"
  readonly name: string
  readonly mimeType: string
  readonly size: number
  readonly url: string
}

/**
 * What one message may carry — the message contract's bounds, which the
 * store's adapter reads off the client (`DRAFT_IMAGE_LIMITS`): how many
 * images, how large each may be and all of them together, and in which
 * encodings; and how many other files, and how large they may be together.
 * How heavy an image may be for a model is the gateway's rule, applied when
 * it stores one.
 */
export interface DraftImageLimits {
  readonly images: number
  readonly bytes: number
  readonly totalBytes: number
  readonly types: readonly string[]
  readonly files: number
  readonly fileBytes: number
}

/** Why what was offered to a composer was not taken: none of it was. */
export type DraftImageRefusal =
  | "too-many"
  | "too-large"
  | "too-heavy"
  | "too-many-files"
  | "too-heavy-files"
  | "empty"
  | "unsupported"

/**
 * Whether a draft goes to the model as an image or is placed where the agent
 * works as a file: one of the image encodings a message names is an image,
 * anything that is not an image at all — a PDF, a ZIP, a text file — is a
 * file. Any other image (HEIC, SVG, a camera RAW) is neither: the gateway
 * stores every `image/*` upload as an image, so it is refused as
 * `unsupported` before anything is sent (`draftImagesRefusal`).
 */
export function draftKind(
  mimeType: string,
  limits: Pick<DraftImageLimits, "types">,
): DraftImage["kind"] {
  return limits.types.includes(mimeType) ? "image" : "file"
}

/** Whether `offered` may join `held` on one message: all of them, or none, with why. */
export function draftImagesRefusal(
  held: readonly Pick<DraftImage, "size" | "kind">[],
  offered: readonly Pick<DraftImage, "size" | "kind" | "mimeType">[],
  limits: DraftImageLimits,
): DraftImageRefusal | null {
  if (
    offered.some((draft) => draft.kind === "file" && draft.mimeType.startsWith("image/"))
  )
    return "unsupported"
  // Storage takes no empty file: there is nothing to hand over.
  if (offered.some((draft) => draft.size === 0)) return "empty"
  const all = [...held, ...offered]
  const images = all.filter((draft) => draft.kind === "image")
  const files = all.filter((draft) => draft.kind === "file")
  if (images.length > limits.images) return "too-many"
  if (offered.some((draft) => draft.kind === "image" && draft.size > limits.bytes))
    return "too-large"
  if (sizeOf(images) > limits.totalBytes) return "too-heavy"
  if (files.length > limits.files) return "too-many-files"
  if (sizeOf(files) > limits.fileBytes) return "too-heavy-files"
  return null
}

const sizeOf = (drafts: readonly Pick<DraftImage, "size">[]) =>
  drafts.reduce((sum, draft) => sum + draft.size, 0)

const mediaType = /^[a-z0-9][a-z0-9!#$&^_.+-]*\/[a-z0-9][a-z0-9!#$&^_.+-]*$/

/**
 * The media type a file is uploaded as: what the system said it is, lowercase
 * and without parameters, or `application/octet-stream` when it said nothing
 * the upload contract takes (a file of a kind nobody registered has none).
 */
export function draftMediaType(type: string): string {
  const bare = type.split(";")[0].trim().toLowerCase()
  return bare.length <= 127 && mediaType.test(bare) ? bare : "application/octet-stream"
}

const utf8 = new TextEncoder()

/**
 * A file's name as a message may carry it: never a path or control text, at
 * most 255 UTF-8 bytes, and something when the system gave nothing. Only what
 * the person sees and the agent finds the file under; it never names where
 * the file goes.
 */
export function draftName(name: string, fallback: string): string {
  // eslint-disable-next-line no-control-regex
  let cleaned = name.replace(/[/\\\u0000-\u001f\u007f-\u009f]/gu, "_").trim()
  while (utf8.encode(cleaned).byteLength > 255)
    cleaned = [...cleaned].slice(0, -1).join("")
  return /\P{White_Space}/u.test(cleaned) ? cleaned : fallback
}

/**
 * What a message of attachments and no words says in a title or a preview:
 * images by their count, one file by its name, and once any file is among
 * several, attachments.
 */
export function imagesLine(
  images: readonly Pick<DraftImage, "kind" | "name">[] | undefined,
): string {
  const count = images?.length ?? 0
  if (count === 0) return ""
  // One file is named by its own name, as the gateway names its conversation.
  if (images?.some((draft) => draft.kind === "file"))
    return count === 1 && images[0] ? images[0].name : `${count} attachments`
  return count === 1 ? "Image" : `${count} images`
}
