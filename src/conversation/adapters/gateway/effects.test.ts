import {
  NessaAttachmentError,
  type NessaClient,
  type ConversationView,
} from "@nessa/client"
import { expect, it, vi } from "vitest"
import { AttachmentStagingError } from "../../application/ports"
import { gatewayEffects } from "./effects"
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
it("joins concurrent creation and forwards exact stable submission IDs", async () => {
  const gate = deferred<{ conversationId: string }>()
  const create = vi.fn(() => gate.promise)
  const send = vi.fn(async () => ({
    executionId: "execution",
    requestId: "action",
    disposition: "queued" as const,
  }))
  const effects = gatewayEffects(
    () => ({ conversation: { create, send } }) as unknown as NessaClient,
  )
  const first = effects.create("server")
  const second = effects.create("server")
  expect(create).toHaveBeenCalledOnce()
  gate.resolve({ conversationId: "server" })
  await Promise.all([first, second])
  await effects.send({
    conversationId: "server",
    executionId: "execution",
    actionId: "action",
    text: "exact",
    attachments: [],
  })
  expect(send).toHaveBeenCalledWith("server", "exact", [], {
    executionId: "execution",
    requestId: "action",
  })
})
it("serializes opaque-revision reads, including overlapping manual refreshes", async () => {
  const first = deferred<ConversationView>()
  const read = vi.fn(() => first.promise)
  const effects = gatewayEffects(
    () => ({ conversation: { read } }) as unknown as NessaClient,
  )
  const pending = effects.read("server")
  const following = effects.read("server")
  await Promise.resolve()
  await Promise.resolve()
  expect(read).toHaveBeenCalledOnce()
  first.resolve({ conversationId: "server" } as ConversationView)
  await Promise.all([pending, following])
  expect(read).toHaveBeenCalledTimes(2)
})

/** The original bytes, described as they are uploaded. */
const file = { digest: `sha256:${"ab".repeat(32)}`, mimeType: "image/heic", size: 3 }
/** What the gateway stored them as. */
const stored = {
  digest: `sha256:${"ef".repeat(32)}`,
  mimeType: "image/jpeg" as const,
  size: 2,
}
const ticket = "cd".repeat(32)
const bytes = new Blob(["raw"], { type: "image/heic" })
const owed = { requestId: "r", state: "upload_required", ticket, expiresAtMs: 1 }
function staging(attachments: { begin: unknown; upload?: unknown }) {
  return gatewayEffects(
    () =>
      ({ attachments: { upload: vi.fn(), ...attachments } }) as unknown as NessaClient,
  )
}

it("forwards a message's images with its text, for send and steer alike", async () => {
  const receipt = { executionId: "execution", requestId: "action", disposition: "queued" }
  const send = vi.fn(async () => receipt)
  const steer = vi.fn(async () => receipt)
  const effects = gatewayEffects(
    () => ({ conversation: { send, steer } }) as unknown as NessaClient,
  )
  const submission = {
    conversationId: "server",
    executionId: "execution",
    actionId: "action",
    text: "",
    attachments: [stored],
  }
  await effects.send(submission)
  await effects.steer(submission)
  const ids = { executionId: "execution", requestId: "action" }
  expect(send).toHaveBeenCalledExactlyOnceWith("server", "", [stored], ids)
  expect(steer).toHaveBeenCalledExactlyOnceWith("server", "", [stored], ids)
})

it("uploads nothing when the conversation already holds the bytes, and answers with its reference", async () => {
  const begin = vi.fn(async () => ({ requestId: "r", state: "stored", image: stored }))
  const upload = vi.fn()
  expect(await staging({ begin, upload }).stageAttachment("server", file, bytes)).toEqual(
    stored,
  )
  expect(begin).toHaveBeenCalledExactlyOnceWith("server", file)
  expect(upload).not.toHaveBeenCalled()
})

it("uploads the original bytes under the ticket and answers with what was stored", async () => {
  const begin = vi.fn(async () => owed)
  const upload = vi.fn(async () => stored)
  const reference = await staging({ begin, upload }).stageAttachment(
    "server",
    file,
    bytes,
  )
  expect(upload).toHaveBeenCalledExactlyOnceWith(ticket, {
    mimeType: "image/heic",
    bytes,
  })
  // The gateway's reference, not a restatement of what was uploaded.
  expect(reference).toEqual(stored)
  expect(reference.digest).not.toBe(file.digest)
})

it.each([
  ["unsupported_image", "unsupported-image"],
  ["image_too_large", "too-large"],
  ["ticket_invalid", "unavailable"],
  ["storage_unavailable", "unavailable"],
  ["audit_unavailable", "unavailable"],
  ["unreachable", "unavailable"],
  ["size_mismatch", "rejected"],
  ["digest_mismatch", "rejected"],
  ["unexpected_response", "rejected"],
] as const)("maps an upload that failed as %s to %s", async (code, reason) => {
  const cause = new NessaAttachmentError(code, 400)
  const error = await staging({
    begin: async () => owed,
    upload: () => Promise.reject(cause),
  })
    .stageAttachment("server", file, bytes)
    .catch((error: unknown) => error)
  expect(error).toBeInstanceOf(AttachmentStagingError)
  expect(error).toMatchObject({ reason, cause })
})

it("maps a refused or unanswered begin without reaching the upload", async () => {
  const upload = vi.fn()
  for (const [error, reason] of [
    [new NessaAttachmentError("begin_refused"), "rejected"],
    [new NessaAttachmentError("unreachable"), "unavailable"],
    // The client refusing to describe these bytes at all; asking again changes nothing.
    [new TypeError("Media type must be lowercase, without parameters"), "rejected"],
  ] as const) {
    await expect(
      staging({ begin: () => Promise.reject(error), upload }).stageAttachment(
        "server",
        file,
        bytes,
      ),
    ).rejects.toMatchObject({ reason })
  }
  expect(upload).not.toHaveBeenCalled()
})

it("reports staging with no live connection as unavailable, asking nothing", async () => {
  const begin = vi.fn()
  const offline = gatewayEffects(() => null)
  await expect(offline.stageAttachment("server", file, bytes)).rejects.toMatchObject({
    reason: "unavailable",
  })
  const reconnecting = gatewayEffects(
    () =>
      ({
        connectionState: { status: "reconnecting" },
        attachments: { begin },
      }) as unknown as NessaClient,
  )
  await expect(
    reconnecting.stageAttachment("server", file, bytes),
  ).rejects.toBeInstanceOf(AttachmentStagingError)
  expect(begin).not.toHaveBeenCalled()
})
