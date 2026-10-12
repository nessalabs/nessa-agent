import { NessaHeldFileError, type AttachmentApi } from "@nessa/client"
import { HeldFileReadError, type HeldFileReader } from "../../application/ports"
import type { HeldFileFailure } from "../../model"

/**
 * The client's codes for a file it did not hand back, as the panel's words.
 * Total over the client's codes, so a code added there does not compile here
 * until somebody decides what a person is told about it.
 */
const failures: Record<NessaHeldFileError["code"], HeldFileFailure> = {
  not_held: "not-held",
  conversation_deleted: "deleted",
  integrity: "damaged",
  // Everything else is "not now": a gateway that is busy, could not read it,
  // does not know the conversation or the caller, is not reached, or answered
  // something this window does not believe. None of them is the file's fault,
  // and none is certain to last.
  conversation_not_found: "unavailable",
  temporarily_unavailable: "unavailable",
  unavailable: "unavailable",
  invalid_request: "unavailable",
  forbidden: "unavailable",
  conversations_not_configured: "unavailable",
  aborted: "unavailable",
  unreachable: "unavailable",
  unexpected_response: "unavailable",
}

/**
 * `waiting`, or a rejection as soon as `signal` aborts: a client still
 * connecting may take as long as its connection does, and a file nobody
 * shows any more does not wait for it.
 */
function unlessAborted<T>(waiting: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) return Promise.reject(signal.reason)
  return new Promise((resolve, reject) => {
    const abandon = () => reject(signal.reason)
    signal.addEventListener("abort", abandon, { once: true })
    waiting
      .then(resolve, reject)
      .finally(() => signal.removeEventListener("abort", abandon))
  })
}

/**
 * Held files read back through the client's verified fetch
 * (`attachments.fetch`), on whichever client `attachments` answers with now.
 * The one place the client's refusals become the panel's
 * {@link HeldFileFailure}; the desktop window reads through this too. A read
 * aborted while the client is still connecting stops at once.
 */
export function heldFileReader(
  attachments: () => AttachmentApi | Promise<AttachmentApi>,
): HeldFileReader {
  return async (conversationId, file, signal) => {
    try {
      const api = await unlessAborted(Promise.resolve().then(attachments), signal)
      return await api.fetch(
        conversationId,
        { digest: file.digest, mimeType: file.mimeType, size: file.size },
        { signal },
      )
    } catch (error) {
      if (error instanceof NessaHeldFileError)
        throw new HeldFileReadError(failures[error.code], error)
      // No client to ask (`ConversationUnavailableError`), a client refusing
      // its own arguments (`TypeError`: a name no view could have given), or
      // a fault: not now, and not the file's doing. The cause is kept.
      throw new HeldFileReadError("unavailable", error)
    }
  }
}
