import type { UploadFailure } from "../../model"

/**
 * What to tell somebody about an upload that did not finish, and whether to
 * offer them a retry.
 *
 * One owner for both: the tile, the notification above the composer, and anything else
 * that reports a failed upload read the same sentence for the same reason, so
 * two surfaces cannot drift into saying different things about the same fact.
 * The reasons themselves are {@link UploadFailure}, which is where they are
 * explained.
 */
export function uploadFailureText(reason: UploadFailure): string {
  switch (reason) {
    case "unreadable":
      return "it could not be read"
    case "unsupported-image":
      return "the gateway could not read this image format"
    case "too-large":
      return "it could not be brought under this model's limits"
    case "image-input-unsupported":
      return "this agent's model does not take images"
    case "conversation-deleted":
      return "this conversation was deleted"
    case "busy":
      return "the gateway was busy with other uploads"
    case "interrupted":
      return "the upload was cut off or timed out"
    case "unavailable":
      return "the gateway could not be reached"
    case "rejected":
      return "the gateway refused it"
  }
}

/**
 * The same reasons as a whole short sentence, for a notification that has room
 * for one. The tile's tooltip keeps the longer clause above.
 */
export function uploadFailureSummary(reason: UploadFailure): string {
  switch (reason) {
    case "unreadable":
      return "The file couldn't be read."
    case "unsupported-image":
      return "That image format isn't supported."
    case "too-large":
      return "The image is too large for this model."
    case "image-input-unsupported":
      return "This agent's model doesn't take images."
    case "conversation-deleted":
      return "This conversation was deleted."
    case "busy":
      return "The gateway is busy. Try again in a moment."
    case "interrupted":
      return "The upload was interrupted."
    case "unavailable":
      return "Couldn't reach the gateway."
    case "rejected":
      return "The gateway refused the image."
  }
}

/**
 * Whether trying the same bytes again could end differently. The gateway's
 * verdicts on the image, and on an agent whose model takes none, are about the
 * image and the agent, so they will be the same next time, as a deleted
 * conversation stays deleted; everything else might not be.
 */
export function worthRetrying(reason: UploadFailure): boolean {
  return (
    reason !== "unsupported-image" &&
    reason !== "too-large" &&
    reason !== "image-input-unsupported" &&
    reason !== "conversation-deleted"
  )
}
