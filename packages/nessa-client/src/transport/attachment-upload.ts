import type {
  AttachmentUploadReply,
  AttachmentUploadTransport,
} from "../application/attachment-upload.js"

/** The part of `fetch` this adapter uses, so a test passes a function and nothing else. */
export type UploadFetch = (
  url: string,
  init: {
    method: "PUT"
    headers: Record<string, string>
    body: Blob
    credentials: "omit"
    redirect: "error"
    signal?: AbortSignal
  },
) => Promise<{ status: number; text(): Promise<string> }>

const TICKET_HEADER = "x-nessa-upload-ticket"
// Both answers are one small JSON object: a stored reference, or `{"code":"…"}`.
// Anything much longer is not one of ours, and is not parsed.
const MAX_ANSWER_CHARS = 1024

/**
 * `PUT /attachments` over `fetch`.
 *
 * The ticket is the whole authority for this request, so nothing else rides
 * along: no cookies, and a redirect is an error rather than a second origin
 * being handed the ticket. The body is the Blob itself — the runtime streams it
 * and sets its length; it is never read into a string here.
 *
 * This only fetches and parses. Whether a body is a stored reference, a refusal,
 * or neither is the caller's decision, so a body that is not small JSON comes
 * back as `undefined` rather than as a guess.
 */
export function fetchAttachmentUpload(
  url: string,
  fetch: UploadFetch,
): AttachmentUploadTransport {
  return {
    async put({ ticket, mimeType, bytes, signal }): Promise<AttachmentUploadReply> {
      const response = await fetch(url, {
        method: "PUT",
        headers: { [TICKET_HEADER]: ticket, "content-type": mimeType },
        body: bytes,
        credentials: "omit",
        redirect: "error",
        signal,
      })
      return { status: response.status, body: await jsonBody(response) }
    },
  }
}

async function jsonBody(response: { text(): Promise<string> }): Promise<unknown> {
  try {
    const text = await response.text()
    return text.length > MAX_ANSWER_CHARS ? undefined : (JSON.parse(text) as unknown)
  } catch {
    return undefined
  }
}
