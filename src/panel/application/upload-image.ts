import type { UploadFailure } from "../../conversation"

/**
 * Everything one upload touches, handed in.
 *
 * The bytes belong to the panel's resource store, and the upload state and the
 * gateway to the conversation. This module owns only the order, so a test
 * drives it with plain functions and deferred promises.
 */
export type UploadPorts = {
  /** `sha256:` and 64 lowercase hex digits, over exactly these bytes. */
  digest(bytes: Blob): Promise<string>
  /** The file's bytes, or nothing once its tile has been removed. */
  bytes(fileId: string): Blob | undefined
  /** Ask the conversation for one step of this file's upload state. */
  change(
    change: { fileId: string } & (
      { to: "uploading" } | { to: "failed"; reason: UploadFailure }
    ),
  ): void
  /**
   * Upload the bytes. Records on the file what the gateway stored them as, or
   * why it did not; never rejects.
   */
  stage(input: {
    conversationId: string
    fileId: string
    file: { digest: string; mimeType: string; size: number }
    bytes: Blob
  }): Promise<void>
}

/**
 * Take one attached image from "just attached" to "the gateway holds it": hash
 * the original bytes, then stage them.
 *
 * The original is what goes. This window does not scale, convert, or compress
 * anything: what a model takes is the gateway's knowledge, so the gateway does
 * that work and answers with the reference a message will name. The digest
 * computed here only identifies the upload, and the only size this window
 * checks is the one it already checked at attach time.
 *
 * Removal is checked after the wait. A tile taken away mid-upload has had its
 * bytes released, and the step after that finds nothing and stops: nothing is
 * staged, and no state is written for a file that is no longer there. Every
 * other ending is an upload state on the file; this never rejects.
 */
export async function uploadImage(
  file: { conversationId: string; id: string; mimeType: string },
  ports: UploadPorts,
): Promise<void> {
  ports.change({ fileId: file.id, to: "uploading" })
  const fail = (reason: UploadFailure) =>
    ports.change({ fileId: file.id, to: "failed", reason })
  const bytes = ports.bytes(file.id)
  if (!bytes || bytes.size < 1) return fail("unreadable")
  try {
    const digest = await ports.digest(bytes)
    if (ports.bytes(file.id) === undefined) return
    await ports.stage({
      conversationId: file.conversationId,
      fileId: file.id,
      file: { digest, mimeType: file.mimeType, size: bytes.size },
      bytes,
    })
  } catch {
    // Hashing or a port threw. The bytes never reached the gateway, and the
    // only thing known about why is that this window could not process them.
    fail("unreadable")
  }
}

/** Why an upload failed, for somebody deciding whether to retry or remove. */
export function uploadFailureText(reason: UploadFailure): string {
  switch (reason) {
    case "unreadable":
      return "it could not be read"
    case "unsupported-image":
      return "the gateway could not read this image format"
    case "too-large":
      return "it could not be brought under this model's limits"
    case "unavailable":
      return "the gateway could not be reached"
    case "rejected":
      return "the gateway refused it"
  }
}

/**
 * Whether trying the same bytes again could end differently. The gateway's two
 * verdicts on an image are about the image, so they will be the same next time;
 * everything else might not be.
 */
export function worthRetrying(reason: UploadFailure): boolean {
  return reason !== "unsupported-image" && reason !== "too-large"
}

/**
 * The one line about the draft's files that belongs above the composer, or null.
 *
 * Said at attach time rather than saved for send: a failed upload names itself,
 * an agent that takes no images is reported as soon as the gateway has said so,
 * and a file no message can carry is called that while it can still be swapped.
 * `imageInput` undefined means the gateway has not answered yet, which is not a
 * no. Uploads in flight are shown on their tiles and need no sentence.
 */
export function attachmentNotice(input: {
  files: readonly {
    name: string
    image: boolean
    upload: { status: string; reason?: UploadFailure }
  }[]
  imageInput: boolean | undefined
}): string | null {
  const failed = input.files.find((file) => file.upload.status === "failed")
  if (failed?.upload.reason)
    return `"${failed.name}" did not upload: ${uploadFailureText(failed.upload.reason)}. ${
      worthRetrying(failed.upload.reason)
        ? "Retry it from its tile, or remove it."
        : "Remove it to send."
    }`
  const unsupported = input.files.find((file) => !file.image)
  if (unsupported)
    return `"${unsupported.name}" can be previewed but not sent: messages carry images, and no other files yet.`
  if (input.files.length > 0 && input.imageInput === false)
    return "This agent does not take images. They will stay in the draft until removed."
  return null
}
