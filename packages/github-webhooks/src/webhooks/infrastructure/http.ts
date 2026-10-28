import {
  receive,
  type DeliveryStore,
  type ReceiveResult,
} from "../application/receive.js"
import { deliveryIdentity, inboxLimits } from "../domain/delivery.js"
import type { PayloadVerifier } from "./signature.js"

type Dependencies = Readonly<{
  store: DeliveryStore
  verifier: PayloadVerifier
  now: () => Date
  report: (id: string, event: string, result: ReceiveResult) => void
}>

function response(status: number, result: string): Response {
  return Response.json(
    { result },
    {
      status,
      headers: { "Cache-Control": "no-store" },
    },
  )
}

async function boundedBody(request: Request): Promise<Uint8Array | null> {
  if (request.body === null) return new Uint8Array()
  const reader = request.body.getReader()
  const chunks: Uint8Array[] = []
  let size = 0
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      size += value.byteLength
      if (size > inboxLimits.bodyBytes) {
        await reader.cancel()
        return null
      }
      chunks.push(value)
    }
  } finally {
    reader.releaseLock()
  }
  const bytes = new Uint8Array(size)
  let offset = 0
  for (const chunk of chunks) {
    bytes.set(chunk, offset)
    offset += chunk.byteLength
  }
  return bytes
}

export function createWebhookHandler(dependencies: Dependencies) {
  return async (request: Request): Promise<Response> => {
    const path = new URL(request.url).pathname
    if (path === "/health" && request.method === "GET") {
      return response(200, "running")
    }
    if (path !== "/github/webhook") return response(404, "not_found")
    if (request.method !== "POST") return response(405, "method_not_allowed")
    if (
      request.headers.get("content-type")?.split(";")[0].trim() !== "application/json"
    ) {
      return response(415, "json_required")
    }
    const identity = deliveryIdentity(
      request.headers.get("x-github-delivery"),
      request.headers.get("x-github-event"),
    )
    if (identity === null) return response(400, "invalid_delivery_headers")
    let bytes: Uint8Array | null
    try {
      bytes = await boundedBody(request)
    } catch {
      return response(400, "body_unreadable")
    }
    if (bytes === null) return response(413, "body_too_large")
    try {
      if (
        !(await dependencies.verifier.verify(
          bytes,
          request.headers.get("x-hub-signature-256"),
        ))
      ) {
        return response(401, "invalid_signature")
      }
      let payload: string
      let parsed: unknown
      try {
        payload = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes)
        parsed = JSON.parse(payload)
      } catch {
        return response(400, "invalid_json")
      }
      if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
        return response(400, "object_required")
      }
      const result = await receive(
        {
          identity,
          payload,
          payloadSha256: await dependencies.verifier.digest(bytes),
          receivedAt: dependencies.now().toISOString(),
          bytes: bytes.byteLength,
        },
        dependencies.store,
      )
      dependencies.report(identity.id, identity.event, result)
      const status = {
        stored: 202,
        duplicate: 200,
        conflict: 409,
        full: 503,
        unavailable: 503,
      } satisfies Record<ReceiveResult, number>
      if (!Object.hasOwn(status, result)) return response(503, "unavailable")
      return response(status[result], result)
    } catch {
      return response(503, "unavailable")
    }
  }
}
