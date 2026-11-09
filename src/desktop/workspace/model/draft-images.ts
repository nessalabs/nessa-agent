/**
 * An image put on a message before it is sent — pasted into a composer — by
 * what the window shows of it and where its bytes are.
 *
 * `url` is an object URL over the bytes the paste carried. It is both the
 * tile's picture and, at send, what the upload reads back, so the state holds
 * strings and never a `Blob`. It lives exactly as long as the workspace names
 * it, in a composer or on a message not yet in its conversation
 * (`releasedImageUrls`, which the store's effects revoke, lets it go after).
 */
export interface DraftImage {
  readonly id: string
  readonly name: string
  readonly mimeType: string
  readonly size: number
  readonly url: string
}

/**
 * What images one message may carry: how many, how large each may be and all
 * of them together, and in which encodings — the message contract's bounds,
 * which the store's adapter reads off the client (`DRAFT_IMAGE_LIMITS`). How
 * heavy an image may be for a model is the gateway's rule, applied when it
 * stores one.
 */
export interface DraftImageLimits {
  readonly images: number
  readonly bytes: number
  readonly totalBytes: number
  readonly types: readonly string[]
}

/** Why images offered to a composer were not taken: none of them were. */
export type DraftImageRefusal = "too-many" | "too-large" | "too-heavy" | "unsupported"

/** Whether `offered` may join `held` on one message: all of them, or none, with why. */
export function draftImagesRefusal(
  held: readonly Pick<DraftImage, "size">[],
  offered: readonly Pick<DraftImage, "size" | "mimeType">[],
  limits: DraftImageLimits,
): DraftImageRefusal | null {
  if (offered.some((image) => !limits.types.includes(image.mimeType)))
    return "unsupported"
  if (held.length + offered.length > limits.images) return "too-many"
  if (offered.some((image) => image.size > limits.bytes)) return "too-large"
  const total = [...held, ...offered].reduce((sum, image) => sum + image.size, 0)
  if (total > limits.totalBytes) return "too-heavy"
  return null
}

/** Whether a pasted file is one a composer takes as an image. */
export function pastedImage(file: { readonly type: string }): boolean {
  return file.type.startsWith("image/")
}

/** What a message of images and no words says in a title or a preview. */
export function imagesLine(images: readonly unknown[] | undefined): string {
  const count = images?.length ?? 0
  return count === 0 ? "" : count === 1 ? "Image" : `${count} images`
}
