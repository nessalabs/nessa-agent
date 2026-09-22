import type { MessageContent } from "./content"

/**
 * Why an upload did not finish, as something to branch on.
 *
 * `unreadable`: the bytes could not be read or hashed in this window.
 * `unsupported-image`: the gateway could not read this image format.
 * `too-large`: the gateway could not bring it under the selected model's limits.
 * `image-input-unsupported`: the agent's model does not take images at all.
 * `busy`: the gateway had no room for another upload just now; it will.
 * `interrupted`: the transfer was cut off or timed out before it finished.
 * `unavailable`: the gateway or its storage could not be reached; trying again may work.
 * `rejected`: the gateway answered and refused these bytes for another reason.
 */
export type UploadFailure =
  | "unreadable"
  | "unsupported-image"
  | "too-large"
  | "image-input-unsupported"
  | "busy"
  | "interrupted"
  | "unavailable"
  | "rejected"

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
 * A file that is not an image stays `not-started`, and always will: nothing
 * uploads it, because nothing needs to. Its bytes are already on the machine
 * the agent runs on, so the message names where it is instead — see
 * {@link FileAttachment.path} and {@link messageFiles}. Only `stored` carries a
 * reference, and it carries the whole one, so there is no way to write down an
 * upload that finished without saying what it finished as.
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
  /** Empty when this window holds no bytes to paint, which a path-only file does not. */
  previewUrl: string
  upload: UploadState
  /**
   * Where the file is on this machine, when the host said so, and `null` when
   * nobody could: a browser `File` deliberately does not carry one, so a file
   * dropped or chosen outside the desktop app has no path at all.
   *
   * This is what makes a file that is not an image sendable. The panel never
   * opens it; the path travels to the gateway, which checks that it can be
   * said and puts it in the prompt as a link the agent may open. See
   * {@link messageFiles}.
   */
  path: string | null
}

/**
 * One file a message points the agent at, by path rather than by content.
 * The same shape the wire carries, and the whole of it: a name beside the path
 * would be a second thing that could disagree with it.
 */
export type LinkedFile = { path: string }

/**
 * An image known only by reference: a turn read back from the gateway, which
 * this window never held the bytes for. It has nothing to preview.
 */
export type ImageReferencePart = { type: "image-reference" } & ImageReference

/**
 * A file a turn pointed the agent at, read back from the gateway: after a
 * reload, or a turn sent from another surface. There is nothing to preview and
 * never was — the path is the whole of what the message carried.
 */
export type LinkedFileReferencePart = { type: "file-reference"; path: string }

/**
 * Preview budgets: what one window will hold. They bound drafts, not messages.
 * The per-file figure is also what the upload path takes — large enough for a
 * camera RAW file — and it is the only byte limit this window puts on a single
 * file: how heavy an image may be for a model is the gateway's rule, applied
 * when it stores one. All of these are MiB, and the panel says "MiB".
 */
export const MAX_ATTACHMENT_BYTES = 64 * 1024 * 1024
export const MAX_DRAFT_ATTACHMENT_BYTES = 128 * 1024 * 1024
export const MAX_DRAFT_ATTACHMENTS = 20
/**
 * How many bytes of already-sent originals stay around to paint the transcript.
 * Past it the oldest sent images fall back to their reference tile, so sending
 * can never use up what attaching needs. One file of the largest size fits.
 */
export const MAX_SENT_PREVIEW_BYTES = 64 * 1024 * 1024

/** Message rules, counted over stored references. The gateway enforces the same. */
export const MAX_SEND_IMAGES = 10
// Ten, not twenty: the agent is handed base64, a third larger, in one 16 MiB frame.
export const MAX_SEND_TOTAL_IMAGE_BYTES = 10 * 1024 * 1024
/**
 * How many files one message may point at. No byte budget stands beside it:
 * nothing about them travels except the paths.
 */
export const MAX_SEND_FILES = 10
/**
 * Longest path a message may name, in UTF-8 bytes — `PATH_MAX`, which is a
 * byte limit on every filesystem this runs on. Counted in bytes everywhere it
 * is counted: here, in the client, and in the gateway's own domain. The
 * published schema's `maxLength` is in code points and so is only a coarse
 * upper bound, which can never refuse a path this accepts.
 */
export const MAX_FILE_PATH_BYTES = 4096

const digestPattern = /^sha256:[0-9a-f]{64}$/

/**
 * Enforce per-file and aggregate draft budgets before changing stored content.
 *
 * The byte budgets are counted over the files this window is holding the bytes
 * of, and over those alone. A file known by path is not one of them: nothing
 * was read and nothing is retained, so a video far past every figure here is an
 * ordinary attachment. What still applies to all of them is how many one draft
 * may hold, which is about the draft rather than about memory, and that every
 * file's size is a number that could be one.
 */
export function validDraftAttachments(content: MessageContent): boolean {
  const files = content.filter((part) => part.type === "file")
  const held = files.filter((file) => !linkedFile(file))
  return (
    files.length <= MAX_DRAFT_ATTACHMENTS &&
    new Set(files.map((file) => file.id)).size === files.length &&
    files.every((file) => Number.isSafeInteger(file.size) && file.size >= 0) &&
    held.every((file) => file.size <= MAX_ATTACHMENT_BYTES) &&
    held.reduce((bytes, file) => bytes + file.size, 0) <= MAX_DRAFT_ATTACHMENT_BYTES
  )
}

/**
 * Images by file extension, for every route that has a name and no bytes.
 *
 * There are two of those. A browser reports most camera RAW files, and
 * sometimes HEIC, with no type at all. And a file chosen through the host's
 * picker was never opened by anything, so its name is the only evidence there
 * is — which is why the ordinary web encodings are listed here too, though a
 * browser always types those itself. The gateway only needs to be told "this is
 * an image"; it reads the real encoding from the bytes.
 */
/** What a file is called when nothing knows any better. */
const FALLBACK_MEDIA_TYPE = "application/octet-stream"

const IMAGE_TYPE_BY_EXTENSION: Readonly<Record<string, string>> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  svg: "image/svg+xml",
  heic: "image/heic",
  heif: "image/heif",
  avif: "image/avif",
  jxl: "image/jxl",
  psd: "image/vnd.adobe.photoshop",
  tif: "image/tiff",
  tiff: "image/tiff",
  bmp: "image/bmp",
  dng: "image/x-adobe-dng",
  cr2: "image/x-canon-cr2",
  cr3: "image/x-canon-cr3",
  nef: "image/x-nikon-nef",
  arw: "image/x-sony-arw",
  raf: "image/x-fuji-raf",
  orf: "image/x-olympus-orf",
  rw2: "image/x-panasonic-rw2",
  pef: "image/x-pentax-pef",
  srw: "image/x-samsung-srw",
}

/**
 * The media type to declare for an attached file. The browser's own answer
 * wins whenever it gave one. When it gave none — empty, or the
 * `application/octet-stream` that means the same — a known image extension
 * decides, so a RAW file is an image here and not a file that cannot be sent.
 *
 * This is what decides a file's route, and the route belongs to the file and
 * never to the gesture: an image is uploaded, anything else is pointed at by
 * path, and that holds whether it arrived through the picker, a drop, or a
 * paste.
 */
export function declaredMediaType(name: string, browserType: string): string {
  const reported = browserType.trim().toLowerCase()
  if (reported && reported !== "application/octet-stream") return reported
  const extension = /\.([a-z0-9]+)$/i.exec(name.trim())?.[1]?.toLowerCase()
  // `Object.hasOwn`, not a bare index: every object inherits `constructor`,
  // `toString` and the rest, so `photo.constructor` used to look up a function
  // here and hand it on as a media type, which threw on the first `startsWith`.
  const known = extension && Object.hasOwn(IMAGE_TYPE_BY_EXTENSION, extension)
  return (known ? IMAGE_TYPE_BY_EXTENSION[extension] : undefined) ?? FALLBACK_MEDIA_TYPE
}

/**
 * Whether this window can paint the file itself. HEIC, RAW, TIFF and the like
 * are images the gateway can read and a webview cannot, so their tiles are a
 * labelled placeholder rather than a broken picture.
 */
export function previewableImage(mimeType: string): boolean {
  return [
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/svg+xml",
    "image/bmp",
    "image/avif",
  ].includes(mimeType)
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
  | { kind: "too-many-files" }

/**
 * The images this content would send, or the one reason it cannot.
 *
 * All or nothing: a message that silently left one file behind would be a
 * different message from the one on screen. What goes is each file's stored
 * reference — never anything derived from the file itself — and a stored
 * reference that is not one names nothing the gateway holds, so it counts as an
 * upload that failed.
 */
/**
 * The references this content already names: each file's stored one, and each
 * part that is nothing but a reference. A file still uploading names none, so
 * whether this is the whole message is {@link messageImages}'s question.
 */
export function storedImages(content: MessageContent): ImageReference[] {
  return content.flatMap((part): ImageReference[] => {
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
}

/**
 * Whether this file travels as a path rather than as bytes: anything that is
 * not an image and that the host gave a path for. An image is never one of
 * these, even when its path is known — an image is carried, so that the model
 * sees it without having to decide to look.
 */
export function linkedFile(file: FileAttachment): boolean {
  return !isImageFile(file.mimeType) && file.path !== null
}

/**
 * Whether a path can be carried to the agent, which is the gateway's rule
 * stated here so that a file it would refuse is turned away while it can still
 * be swapped, rather than after somebody has pressed send.
 *
 * Every component below the root has to be a name: absolute, no control
 * character in it, and none empty, `.` or `..`, none of which survive the path
 * being written as a URI. The gateway keeps the same rule in its domain and
 * the protocol publishes it; `adapters/gateway/effects.ts` is where the three
 * are held to each other, because this model does not import a client SDK.
 *
 * Square brackets used to be refused here too, because the agent is handed the
 * path inside a markdown link and a bracket could close it. That rule covered
 * one of the three characters that can, and is gone rather than extended: the
 * gateway's ACP adapter now encodes both halves of the link down to an
 * allowlist, so `[draft] notes.pdf` is an ordinary name and nothing downstream
 * depends on its absence.
 */
export function linkablePath(path: string): boolean {
  // Bytes, because that is what the gateway counts and what the constant says.
  // Counting UTF-16 code units — `path.length` — let a path of 4,095 CJK
  // characters through here at 12,286 bytes, to be refused after it was sent
  // as `invalid_request`, which the panel deliberately shows no sentence for.
  if (!path.startsWith("/")) return false
  if (new TextEncoder().encode(path).length > MAX_FILE_PATH_BYTES) return false
  return path
    .slice(1)
    .split("/")
    .every(
      (part) =>
        part.length > 0 &&
        part !== "." &&
        part !== ".." &&
        ![...part].some(controlCharacter),
    )
}

/**
 * C0, DEL and C1 — the same set the gateway's own check covers. Written by
 * code point rather than as a pattern: a regular expression holding literal
 * control characters is unreadable and one written with escapes is easy to get
 * a range wrong in, which is how C1 came to be missing from the published rule
 * while the gateway refused it.
 */
function controlCharacter(character: string): boolean {
  const code = character.codePointAt(0) ?? 0
  return code <= 0x1f || (code >= 0x7f && code <= 0x9f)
}

/**
 * The files this content points the agent at, in attachment order. Nothing
 * here is uploaded and nothing here is read; whether there are too many of
 * them is {@link messageImages}'s question, which answers for the whole
 * message at once.
 */
export function messageFiles(content: MessageContent): LinkedFile[] {
  return content.flatMap((part) => {
    if (part.type === "file-reference") return [{ path: part.path }]
    return part.type === "file" && part.path !== null && linkedFile(part)
      ? [{ path: part.path }]
      : []
  })
}

export function messageImages(
  content: MessageContent,
): { ok: true; images: ImageReference[] } | { ok: false; refusal: ImageRefusal } {
  const parts = content.filter((part) => part.type === "file")
  // What travels as a path is not waited for, not uploaded, and not an image.
  const files = parts.filter((file) => !linkedFile(file))
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
  const images = storedImages(content)
  if (images.length > MAX_SEND_IMAGES)
    return { ok: false, refusal: { kind: "too-many-images" } }
  if (images.reduce((bytes, image) => bytes + image.size, 0) > MAX_SEND_TOTAL_IMAGE_BYTES)
    return { ok: false, refusal: { kind: "images-too-large" } }
  // Counted the same way the message counts them, so a turn read back from the
  // gateway — whose files are references rather than draft parts — is held to
  // the same bound as one attached here.
  if (messageFiles(content).length > MAX_SEND_FILES)
    return { ok: false, refusal: { kind: "too-many-files" } }
  return { ok: true, images }
}

/**
 * Bytes as a person reads them: "812 KiB", "3.4 MiB". Binary units, named as
 * such: every budget here is a power of two, and "MB" beside one would be a
 * different number.
 */
export function humanSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KiB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`
}

/** What stands in for an image this window cannot show: "PNG image, 812 KiB". */
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
