import { expect, it, vi } from "vitest"

import {
  NessaAttachmentError,
  type AttachmentUploadTransport,
} from "../application/attachment-upload.js"
import { NessaRpcError } from "../application/rpc-error.js"
import { createAttachmentApi } from "./attachment-api.js"

const conversationId = "00000000-0000-4000-8000-000000000001"
const digest = `sha256:${"ab".repeat(32)}`
const ticket = "cd".repeat(32)
/** What is uploaded: the original bytes, which need not be anything a message carries. */
const file = { digest, mimeType: "image/heic", size: 3 }
/** What the gateway stored them as: normalized, so every field may differ. */
const stored = { digest: `sha256:${"ef".repeat(32)}`, mimeType: "image/jpeg", size: 2 }
const bytes = new Blob(["raw"], { type: "image/heic" })

const owed = { state: "upload_required", ticket, expiresAtMs: 1_700_000_000_000 }
const noTicket = { ticket: null, expiresAtMs: null }
const noReference = { digest: null, mimeType: null, size: null }

const unused: AttachmentUploadTransport = {
  put: () => Promise.reject(new Error("no upload expected")),
}
function api(request: (method: string, params: unknown) => Promise<unknown>) {
  return createAttachmentApi({ request }, unused, () => "generated")
}

it("begins with exactly the described bytes and returns the ticket", async () => {
  const request = vi.fn(async () => ({ requestId: "attach", ...owed, ...noReference }))
  const beginning = await api(request).begin(conversationId, file, {
    requestId: "attach",
  })
  expect(request).toHaveBeenCalledExactlyOnceWith("attachment.begin", {
    conversationId,
    requestId: "attach",
    ...file,
  })
  expect(beginning).toEqual({ requestId: "attach", ...owed })
})

it("reports bytes the conversation already holds as the reference to send, with no ticket", async () => {
  const request = vi.fn(async () => ({
    requestId: "generated",
    state: "stored",
    ...noTicket,
    ...stored,
  }))
  expect(await api(request).begin(conversationId, file)).toEqual({
    requestId: "generated",
    state: "stored",
    // The gateway's reference, not the description that was asked about.
    image: stored,
  })
})

it.each([
  ["stored with a ticket", { state: "stored", ticket, expiresAtMs: null, ...stored }],
  ["stored with an expiry", { state: "stored", ticket: null, expiresAtMs: 1, ...stored }],
  ["stored with no reference", { state: "stored", ...noTicket, ...noReference }],
  [
    "stored with half a reference",
    { state: "stored", ...noTicket, ...stored, size: null },
  ],
  [
    "stored as something no message carries",
    { state: "stored", ...noTicket, ...stored, mimeType: "image/heic" },
  ],
  [
    "stored under a malformed digest",
    { state: "stored", ...noTicket, ...stored, digest: digest.toUpperCase() },
  ],
  ["stored as nothing", { state: "stored", ...noTicket, ...stored, size: 0 }],
  ["a ticket owed and absent", { ...owed, ticket: null, ...noReference }],
  ["a malformed ticket", { ...owed, ticket: "CD", ...noReference }],
  ["a ticket with no expiry", { ...owed, expiresAtMs: null, ...noReference }],
  ["a ticket beside a stored reference", { ...owed, ...stored }],
  ["a ticket beside part of a reference", { ...owed, ...noReference, digest }],
  ["an unknown state", { state: "pending", ...noTicket, ...noReference }],
  ["an unknown field", { state: "stored", ...noTicket, ...stored, url: "x" }],
  ["another action's answer", { requestId: "other", ...owed, ...noReference }],
])("refuses a begin reply that is %s", async (_name, reply) => {
  const request = async () => ({ requestId: "attach", ...reply })
  const error = await api(request)
    .begin(conversationId, file, { requestId: "attach" })
    .catch((error: unknown) => error)
  expect(error).toBeInstanceOf(NessaAttachmentError)
  expect(error).toMatchObject({ code: "unexpected_response" })
  // The ticket is a secret; a malformed reply is no reason to print one.
  expect(String((error as Error).message)).not.toContain(ticket)
  expect(String(((error as Error).cause as Error).message)).not.toContain(ticket)
})

it("reads a field that is absent the way it reads one that is null", async () => {
  expect(
    await api(async () => ({ requestId: "generated", ...owed })).begin(
      conversationId,
      file,
    ),
  ).toMatchObject({ state: "upload_required", ticket })
  expect(
    await api(async () => ({ requestId: "generated", state: "stored", ...stored })).begin(
      conversationId,
      file,
    ),
  ).toMatchObject({ state: "stored", image: stored })
})

it("tells a gateway refusal from no answer by type", async () => {
  const refused = await api(() =>
    Promise.reject(new NessaRpcError("invalid_request", "unreachable")),
  )
    .begin(conversationId, file)
    .catch((error: unknown) => error)
  expect(refused).toMatchObject({ code: "begin_refused" })

  // The same words in a plain error are not a refusal.
  const lost = await api(() => Promise.reject(new Error("invalid_request")))
    .begin(conversationId, file)
    .catch((error: unknown) => error)
  expect(lost).toMatchObject({ code: "unreachable" })
})

it.each([
  ["a non-canonical conversation", ["Conversation", file]],
  ["an uppercase digest", [conversationId, { ...file, digest: digest.toUpperCase() }]],
  [
    "a media type with parameters",
    [conversationId, { ...file, mimeType: "image/png; q=1" }],
  ],
  ["an uppercase media type", [conversationId, { ...file, mimeType: "Image/PNG" }]],
  ["no bytes", [conversationId, { ...file, size: 0 }]],
  ["more than storage takes", [conversationId, { ...file, size: 20 * 1024 * 1024 + 1 }]],
] as const)("refuses to begin for %s before asking the gateway", async (_name, args) => {
  const request = vi.fn()
  await expect(api(request).begin(args[0], args[1])).rejects.toBeInstanceOf(TypeError)
  expect(request).not.toHaveBeenCalled()
})

it("accepts any media type up to 20 MiB: what becomes of it is the gateway's decision", async () => {
  const request = vi.fn(async () => ({ requestId: "generated", ...owed, ...noReference }))
  for (const mimeType of ["image/tiff", "image/bmp", "application/pdf"])
    await api(request).begin(conversationId, { digest, mimeType, size: 20 * 1024 * 1024 })
  expect(request).toHaveBeenCalledTimes(3)
})

it("uploads the bytes under the ticket and returns what the gateway stored them as", async () => {
  const put = vi.fn(async () => ({ status: 200, body: stored }))
  const uploads = createAttachmentApi({ request: vi.fn() }, { put }, () => "id")
  const signal = new AbortController().signal
  const reference = await uploads.upload(
    ticket,
    { mimeType: "image/heic", bytes },
    { signal },
  )
  expect(put).toHaveBeenCalledExactlyOnceWith({
    ticket,
    mimeType: "image/heic",
    bytes,
    signal,
  })
  // Normalized: none of digest, media type, or size is what was uploaded.
  expect(reference).toEqual(stored)
  expect(reference.digest).not.toBe(file.digest)
})

it.each([
  ["nothing", undefined],
  ["not an object", "stored"],
  ["a list", [stored]],
  ["missing its size", { digest: stored.digest, mimeType: stored.mimeType }],
  ["an uppercase digest", { ...stored, digest: stored.digest.toUpperCase() }],
  ["a media type no message carries", { ...stored, mimeType: "image/heic" }],
  ["an empty image", { ...stored, size: 0 }],
  ["a fractional size", { ...stored, size: 1.5 }],
  ["more than a whole message may carry", { ...stored, size: 10 * 1024 * 1024 + 1 }],
  ["an unknown field", { ...stored, url: "https://elsewhere" }],
])("refuses a success whose stored reference is %s", async (_name, body) => {
  const uploads = createAttachmentApi(
    { request: vi.fn() },
    { put: async () => ({ status: 200, body }) },
    () => "id",
  )
  const error = await uploads
    .upload(ticket, { mimeType: "image/heic", bytes })
    .catch((error: unknown) => error)
  expect(error).toBeInstanceOf(NessaAttachmentError)
  expect(error).toMatchObject({ code: "unexpected_response", status: 200 })
})

it.each([
  [401, { code: "ticket_invalid" }, "ticket_invalid"],
  [400, { code: "size_mismatch" }, "size_mismatch"],
  [422, { code: "digest_mismatch" }, "digest_mismatch"],
  [415, { code: "unsupported_image" }, "unsupported_image"],
  [413, { code: "image_too_large" }, "image_too_large"],
  [503, { code: "storage_unavailable" }, "storage_unavailable"],
  [503, { code: "audit_unavailable" }, "audit_unavailable"],
  [500, { code: "out_of_cheese" }, "unexpected_response"],
  [400, { code: 7 }, "unexpected_response"],
  [502, undefined, "unexpected_response"],
  // The old success. A reference is the only proof of what was stored.
  [204, undefined, "unexpected_response"],
  // A stored reference under a failing status is not a success.
  [500, stored, "unexpected_response"],
])("maps upload answer %i %j to %s", async (status, body, expected) => {
  const uploads = createAttachmentApi(
    { request: vi.fn() },
    { put: async () => ({ status, body }) },
    () => "id",
  )
  const error = await uploads
    .upload(ticket, { mimeType: "image/heic", bytes })
    .catch((error: unknown) => error)
  expect(error).toBeInstanceOf(NessaAttachmentError)
  expect(error).toMatchObject({ code: expected, status })
  expect(String((error as Error).message)).not.toContain(ticket)
})

it("reports an upload that got no answer as unreachable, keeping the cause", async () => {
  const cause = new TypeError("Failed to fetch")
  const uploads = createAttachmentApi(
    { request: vi.fn() },
    { put: () => Promise.reject(cause) },
    () => "id",
  )
  const error = await uploads
    .upload(ticket, { mimeType: "image/heic", bytes })
    .catch((error: unknown) => error)
  expect(error).toMatchObject({ code: "unreachable", cause })
})

it("refuses a malformed ticket or an empty body before any request", async () => {
  const put = vi.fn()
  const uploads = createAttachmentApi({ request: vi.fn() }, { put }, () => "id")
  await expect(
    uploads.upload("not-a-ticket", { mimeType: "image/png", bytes }),
  ).rejects.toBeInstanceOf(TypeError)
  await expect(
    uploads.upload(ticket, { mimeType: "image/png", bytes: new Blob([]) }),
  ).rejects.toBeInstanceOf(TypeError)
  expect(put).not.toHaveBeenCalled()
})
