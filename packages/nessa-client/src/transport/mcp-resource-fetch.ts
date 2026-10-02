import type {
  McpResourceReply,
  McpResourceTransport,
} from "../application/mcp-resource-fetch.js"

/** The part of `fetch` this adapter uses, so a test passes a function and nothing else. */
export type ResourceFetch = (
  url: string,
  init: {
    method: "GET"
    headers: Record<string, string>
    credentials: "omit"
    redirect: "error"
    cache: "no-store"
    signal?: AbortSignal
  },
) => Promise<{ status: number; body: ReadableStream<Uint8Array> | null }>

const TICKET_HEADER = "x-nessa-resource-ticket"

/**
 * `GET /mcp-resources` over `fetch`.
 *
 * The ticket rides in its header and nowhere else — never the URL, which logs,
 * history, and referrers keep. It is the whole authority for this request, so
 * nothing else rides along: no cookies, no cache, and a redirect is an error
 * rather than a second origin being handed the ticket.
 *
 * The body is read only for a `200`, and only up to one byte past `maxBytes`:
 * an answer longer than the bytes described is already wrong, and there is no
 * reason to hold the rest of it. Whether the bytes are the right ones is the
 * caller's decision.
 */
export function fetchMcpResource(
  url: string,
  fetch: ResourceFetch,
): McpResourceTransport {
  return {
    async get({ ticket, maxBytes, signal }): Promise<McpResourceReply> {
      const response = await fetch(url, {
        method: "GET",
        headers: { [TICKET_HEADER]: ticket },
        credentials: "omit",
        redirect: "error",
        cache: "no-store",
        signal,
      })
      if (response.status !== 200) {
        await response.body?.cancel().catch(() => {})
        return { status: response.status, bytes: new Uint8Array() }
      }
      return { status: 200, bytes: await readAtMost(response.body, maxBytes + 1) }
    },
  }
}

async function readAtMost(
  body: ReadableStream<Uint8Array> | null,
  limit: number,
): Promise<Uint8Array<ArrayBuffer>> {
  if (!body) return new Uint8Array()
  const reader = body.getReader()
  const chunks: Uint8Array[] = []
  let length = 0
  while (length < limit) {
    const { done, value } = await reader.read()
    if (done) break
    chunks.push(value)
    length += value.byteLength
  }
  if (length >= limit) await reader.cancel().catch(() => {})
  const bytes = new Uint8Array(Math.min(length, limit))
  let offset = 0
  for (const chunk of chunks) {
    const part = chunk.subarray(0, bytes.byteLength - offset)
    bytes.set(part, offset)
    offset += part.byteLength
  }
  return bytes
}
