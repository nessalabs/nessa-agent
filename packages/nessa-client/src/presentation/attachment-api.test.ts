import { expect, it, vi } from "vitest"

import {
  NessaAttachmentError,
  UPLOAD_DEADLINE_MS,
  type AttachmentUploadReply,
  type AttachmentUploadTransport,
  type UploadTimer,
} from "../application/attachment-upload.js"
import { NessaRpcError } from "../application/rpc-error.js"
import { asImageAttachment } from "../protocol/attachment-validate.js"
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

/** A deadline clock the test fires by hand. Nothing here waits. */
function manualTimer() {
  const pending: { ms: number; elapsed: () => void; cancelled: boolean }[] = []
  const timer: UploadTimer = (ms, elapsed) => {
    const entry = { ms, elapsed, cancelled: false }
    pending.push(entry)
    return () => {
      entry.cancelled = true
    }
  }
  return { timer, pending }
}
const unused: AttachmentUploadTransport = {
  put: () => Promise.reject(new Error("no upload expected")),
}
function api(request: (method: string, params: unknown) => Promise<unknown>) {
  return createAttachmentApi({ request }, unused, () => "generated", manualTimer().timer)
}
function uploads(put: AttachmentUploadTransport["put"], clock = manualTimer()) {
  return createAttachmentApi({ request: vi.fn() }, { put }, () => "id", clock.timer)
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

it("reports bytes the conversation already holds as what it holds them as, with no ticket", async () => {
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
    stored,
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
    "stored under a media type with parameters",
    { state: "stored", ...noTicket, ...stored, mimeType: "image/png; q=1" },
  ],
  [
    "stored under a malformed digest",
    { state: "stored", ...noTicket, ...stored, digest: digest.toUpperCase() },
  ],
  ["stored as nothing", { state: "stored", ...noTicket, ...stored, size: 0 }],
  [
    "stored as more than the upload path takes",
    { state: "stored", ...noTicket, ...stored, size: 67_108_864 + 1 },
  ],
  ["a ticket owed and absent", { ...owed, ticket: null, ...noReference }],
  ["a malformed ticket", { ...owed, ticket: "CD", ...noReference }],
  ["a ticket with no expiry", { ...owed, expiresAtMs: null, ...noReference }],
  ["a ticket beside a stored reference", { ...owed, ...stored }],
  ["a ticket beside part of a reference", { ...owed, ...noReference, digest }],
  ["an unknown state", { state: "pending", ...noTicket, ...noReference }],
  ["an unknown field", { state: "stored", ...noTicket, ...stored, url: "x" }],
  ["another action's answer", { requestId: "other", ...owed, ...noReference }],
  // One current contract: "not here" is null. A reply that leaves a field out
  // is some other contract, however reasonable the rest of it looks.
  ["missing the reference fields of an owed upload", { ...owed }],
  ["missing the ticket fields of a stored file", { state: "stored", ...stored }],
  ["missing one field", { ...owed, digest: null, mimeType: null }],
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

it.each([
  "invalid_request",
  "conversation_not_found",
  "attachment_capacity",
  "attachment_storage_unavailable",
  "storage_unavailable",
  "audit_unavailable",
  "temporarily_unavailable",
  "agent_not_configured",
  "image_input_unsupported",
] as const)("carries the gateway's reason for refusing to begin: %s", async (code) => {
  // The message says something else entirely: the reason is read from the code.
  const cause = new NessaRpcError(code, "attachment_capacity invalid_request")
  const error = await api(() => Promise.reject(cause))
    .begin(conversationId, file)
    .catch((error: unknown) => error)
  expect(error).toMatchObject({ code: "begin_refused", refusal: code, cause })
})

it("tells a refusal it was not taught, and no answer at all, from the ones it knows", async () => {
  const novel = await api(() =>
    Promise.reject(
      new NessaRpcError("attachment_quota_exceeded", "temporarily_unavailable"),
    ),
  )
    .begin(conversationId, file)
    .catch((error: unknown) => error)
  expect(novel).toMatchObject({ code: "begin_refused", refusal: "unexpected" })

  // The same words in a plain error are not a refusal.
  const lost = await api(() => Promise.reject(new Error("attachment_capacity")))
    .begin(conversationId, file)
    .catch((error: unknown) => error)
  expect(lost).toMatchObject({ code: "unreachable", refusal: undefined })
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
  [
    "more than the upload path takes",
    [conversationId, { ...file, size: 67_108_864 + 1 }],
  ],
] as const)("refuses to begin for %s before asking the gateway", async (_name, args) => {
  const request = vi.fn()
  await expect(api(request).begin(args[0], args[1])).rejects.toBeInstanceOf(TypeError)
  expect(request).not.toHaveBeenCalled()
})

it("accepts any media type up to 64 MiB: what becomes of it is the gateway's decision", async () => {
  const request = vi.fn(async () => ({ requestId: "generated", ...owed, ...noReference }))
  for (const mimeType of ["image/x-canon-cr3", "image/tiff", "application/pdf"])
    await api(request).begin(conversationId, { digest, mimeType, size: 67_108_864 })
  expect(request).toHaveBeenCalledTimes(3)
})

it("uploads the bytes under the ticket and returns what the gateway stored them as", async () => {
  const put = vi.fn(async () => ({ status: 200, body: stored }))
  const clock = manualTimer()
  const reference = await uploads(put, clock).upload(ticket, {
    mimeType: "image/heic",
    bytes,
  })
  expect(put).toHaveBeenCalledExactlyOnceWith({
    ticket,
    mimeType: "image/heic",
    bytes,
    signal: expect.any(AbortSignal),
  })
  // Normalized: none of digest, media type, or size is what was uploaded.
  expect(reference).toEqual(stored)
  expect(reference.digest).not.toBe(file.digest)
  // An answered upload leaves no deadline running behind it.
  expect(clock.pending).toMatchObject([{ ms: UPLOAD_DEADLINE_MS, cancelled: true }])
})

it("returns a stored file of any media type, and narrows to an image as a separate step", async () => {
  const pdf = { digest, mimeType: "application/pdf", size: 40 * 1024 * 1024 }
  const reference = await uploads(async () => ({ status: 200, body: pdf })).upload(
    ticket,
    {
      mimeType: "application/pdf",
      bytes,
    },
  )
  // Storage is media-agnostic: this is a success, not an unexpected response.
  expect(reference).toEqual(pdf)
  expect(asImageAttachment(reference)).toBeUndefined()
  expect(asImageAttachment(stored)).toEqual(stored)
  // The schema's image bound, exactly: 5242880 is an image, one byte more is not.
  expect(asImageAttachment({ ...stored, size: 5_242_880 })).toBeDefined()
  expect(asImageAttachment({ ...stored, size: 5_242_881 })).toBeUndefined()
  expect(asImageAttachment({ ...stored, mimeType: "image/heic" })).toBeUndefined()
})

it.each([
  ["nothing", undefined],
  ["not an object", "stored"],
  ["a list", [stored]],
  ["missing its size", { digest: stored.digest, mimeType: stored.mimeType }],
  ["an uppercase digest", { ...stored, digest: stored.digest.toUpperCase() }],
  ["an uppercase media type", { ...stored, mimeType: "Image/JPEG" }],
  ["an empty file", { ...stored, size: 0 }],
  ["a fractional size", { ...stored, size: 1.5 }],
  ["more than the upload path takes", { ...stored, size: 67_108_864 + 1 }],
  ["an unknown field", { ...stored, url: "https://elsewhere" }],
])("refuses a success whose stored reference is %s", async (_name, body) => {
  const error = await uploads(async () => ({ status: 200, body }))
    .upload(ticket, { mimeType: "image/heic", bytes })
    .catch((error: unknown) => error)
  expect(error).toBeInstanceOf(NessaAttachmentError)
  expect(error).toMatchObject({ code: "unexpected_response", status: 200 })
})

it.each([
  [401, { code: "ticket_invalid" }, "ticket_invalid"],
  [400, { code: "size_mismatch" }, "size_mismatch"],
  [422, { code: "digest_mismatch" }, "digest_mismatch"],
  [400, { code: "upload_interrupted" }, "upload_interrupted"],
  [409, { code: "attachment_not_kept" }, "attachment_not_kept"],
  [503, { code: "upload_unresolved" }, "upload_unresolved"],
  [408, { code: "upload_timeout" }, "upload_timeout"],
  [415, { code: "unsupported_image" }, "unsupported_image"],
  [413, { code: "image_too_large" }, "image_too_large"],
  [415, { code: "image_input_unsupported" }, "image_input_unsupported"],
  [503, { code: "storage_unavailable" }, "storage_unavailable"],
  [503, { code: "audit_unavailable" }, "audit_unavailable"],
  [503, { code: "temporarily_unavailable" }, "temporarily_unavailable"],
  // The refusal stays the refusal when its audit record was lost with it.
  [401, { code: "ticket_invalid", audit: "unavailable" }, "ticket_invalid"],
  // A code nobody taught this client is not passed through as if it were known.
  [500, { code: "out_of_cheese" }, "unexpected_response"],
  [400, { code: 7 }, "unexpected_response"],
  [502, undefined, "unexpected_response"],
  // The old success. A reference is the only proof of what was stored.
  [204, undefined, "unexpected_response"],
  // A stored reference under a failing status is not a success.
  [500, stored, "unexpected_response"],
])("maps upload answer %i %j to %s", async (status, body, expected) => {
  const error = await uploads(async () => ({ status, body }))
    .upload(ticket, { mimeType: "image/heic", bytes })
    .catch((error: unknown) => error)
  expect(error).toBeInstanceOf(NessaAttachmentError)
  expect(error).toMatchObject({ code: expected, status })
  expect(String((error as Error).message)).not.toContain(ticket)
})

it("reports an upload that got no answer as unreachable, keeping the cause", async () => {
  const cause = new TypeError("Failed to fetch")
  const error = await uploads(() => Promise.reject(cause))
    .upload(ticket, { mimeType: "image/heic", bytes })
    .catch((error: unknown) => error)
  expect(error).toMatchObject({ code: "unreachable", cause })
})

it("gives up on a PUT that never answers when its deadline elapses, and aborts the request", async () => {
  const clock = manualTimer()
  let requestSignal: AbortSignal | undefined
  // Never settles, and ignores the abort: the wait must end all the same.
  const put = vi.fn((upload: { signal?: AbortSignal }) => {
    requestSignal = upload.signal
    return new Promise<AttachmentUploadReply>(() => {})
  })
  const pending = uploads(put, clock)
    .upload(ticket, { mimeType: "image/heic", bytes })
    .catch((error: unknown) => error)
  expect(clock.pending).toMatchObject([{ ms: UPLOAD_DEADLINE_MS, cancelled: false }])
  clock.pending[0]!.elapsed()
  const error = await pending
  expect(error).toBeInstanceOf(NessaAttachmentError)
  // No status: nothing answered. The gateway's own 408 carries one.
  expect(error).toMatchObject({ code: "upload_timeout", status: undefined })
  expect(requestSignal?.aborted).toBe(true)
})

it("gives up as upload_timeout when the timer is already out of budget, sending nothing", async () => {
  // The contract says `elapsed` is called once after `ms`, not that a turn of
  // the event loop passes first: a clock a test drives by hand, or one whose
  // budget is already spent, calls it while `timer` is still running.
  const cancelled: boolean[] = []
  const now: UploadTimer = (_ms, elapsed) => {
    elapsed()
    return () => cancelled.push(true)
  }
  const put = vi.fn(() => new Promise<AttachmentUploadReply>(() => {}))
  const error = await createAttachmentApi({ request: vi.fn() }, { put }, () => "id", now)
    .upload(ticket, { mimeType: "image/heic", bytes })
    .catch((error: unknown) => error)
  expect(error).toBeInstanceOf(NessaAttachmentError)
  expect(error).toMatchObject({ code: "upload_timeout", status: undefined })
  expect(put).not.toHaveBeenCalled()
  // The timer is still stopped, so a clock holding a handle lets it go.
  expect(cancelled).toEqual([true])
})

it("reports the caller's own abort as aborted, not as a gateway that cannot be reached", async () => {
  const clock = manualTimer()
  const caller = new AbortController()
  let requestSignal: AbortSignal | undefined
  const put = vi.fn((upload: { signal?: AbortSignal }) => {
    requestSignal = upload.signal
    // What `fetch` does when its signal aborts.
    return new Promise<AttachmentUploadReply>((_, reject) =>
      upload.signal?.addEventListener("abort", () =>
        reject(new DOMException("The operation was aborted.", "AbortError")),
      ),
    )
  })
  const pending = uploads(put, clock)
    .upload(ticket, { mimeType: "image/heic", bytes }, { signal: caller.signal })
    .catch((error: unknown) => error)
  caller.abort()
  expect(await pending).toMatchObject({ code: "aborted" })
  expect(requestSignal?.aborted).toBe(true)
  expect(clock.pending[0]!.cancelled).toBe(true)

  // Already aborted: nothing is sent at all.
  const unsent = vi.fn()
  await expect(
    uploads(unsent).upload(
      ticket,
      { mimeType: "image/heic", bytes },
      { signal: caller.signal },
    ),
  ).rejects.toMatchObject({ code: "aborted" })
  expect(unsent).not.toHaveBeenCalled()
})

it("refuses a malformed ticket or an empty body before any request", async () => {
  const put = vi.fn()
  const api = uploads(put)
  await expect(
    api.upload("not-a-ticket", { mimeType: "image/png", bytes }),
  ).rejects.toBeInstanceOf(TypeError)
  await expect(
    api.upload(ticket, { mimeType: "image/png", bytes: new Blob([]) }),
  ).rejects.toBeInstanceOf(TypeError)
  expect(put).not.toHaveBeenCalled()
})
