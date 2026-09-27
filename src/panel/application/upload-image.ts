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
  /**
   * Ask the conversation for one step of this file's upload state, and hear
   * whether it was taken. A step is refused when the file is not where the step
   * starts from: gone from every draft, or already uploading.
   */
  change(
    change: { fileId: string } & (
      { to: "uploading" } | { to: "failed"; reason: UploadFailure }
    ),
  ): boolean
  /**
   * Upload the bytes. Records on the file what the gateway stored them as, or
   * why it did not; never rejects.
   */
  stage(
    input: {
      conversationId: string
      fileId: string
      file: { digest: string; mimeType: string; size: number }
      bytes: Blob
    },
    signal: AbortSignal,
  ): Promise<void>
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
 * Nothing is sent for a file the conversation did not take to `uploading`: a
 * snapshot a render behind can offer a file that has since gone or already
 * started, and uploading it anyway would spend a gateway slot on a result
 * nobody records.
 *
 * `signal` is aborted when the tile is taken away. Hashing cannot be stopped,
 * so removal is checked after it; the transfer can, and stops. Every other
 * ending is an upload state on the file; this never rejects.
 */
export async function uploadImage(
  file: { conversationId: string; id: string; mimeType: string },
  ports: UploadPorts,
  signal: AbortSignal,
): Promise<void> {
  if (!ports.change({ fileId: file.id, to: "uploading" })) return
  const fail = (reason: UploadFailure) => {
    ports.change({ fileId: file.id, to: "failed", reason })
  }
  const bytes = ports.bytes(file.id)
  if (!bytes || bytes.size < 1) return fail("unreadable")
  try {
    const digest = await ports.digest(bytes)
    if (signal.aborted || ports.bytes(file.id) === undefined) return
    await ports.stage(
      {
        conversationId: file.conversationId,
        fileId: file.id,
        file: { digest, mimeType: file.mimeType, size: bytes.size },
        bytes,
      },
      signal,
    )
  } catch {
    // Hashing or a port threw. The bytes never reached the gateway, and the
    // only thing known about why is that this window could not process them.
    fail("unreadable")
  }
}

/**
 * How many uploads this window keeps in flight. The gateway takes four at once
 * and holds each slot until the image is normalized, so a window that started
 * every upload the moment a dozen images were dropped would have most of them
 * refused for want of room. One slot is left for another window.
 */
export const MAX_UPLOADS_IN_FLIGHT = 3

/**
 * Which images to start uploading now: those not started, in the order they
 * appear, as far as the free slots go. The rest stay `not-started` — waiting,
 * and shown as waiting — and are chosen when a slot frees. Pure, so "the sixth
 * image waits and then goes" is an assertion rather than a timing.
 */
export function nextUploads<T extends { id: string }>(
  waiting: readonly T[],
  inFlight: ReadonlySet<string>,
  limit = MAX_UPLOADS_IN_FLIGHT,
): T[] {
  const free = Math.max(0, limit - inFlight.size)
  return waiting.filter((file) => !inFlight.has(file.id)).slice(0, free)
}
