import {
  IMAGE_ATTACHMENT_TYPES,
  MAX_IMAGE_ATTACHMENT_BYTES,
  MAX_MESSAGE_IMAGE_BYTES,
  MAX_MESSAGE_IMAGES,
  MAX_UPLOAD_BYTES,
  NessaAttachmentError,
  NessaConversationMutationError,
  NessaRpcError,
  type NessaClient,
  type ConversationView,
} from "@nessa/client"
import { expect, it, vi } from "vitest"
import { AttachmentStagingError, SubmissionRefusedError } from "../../application/ports"
import {
  MAX_ATTACHMENT_BYTES,
  MAX_SEND_IMAGES,
  MAX_SEND_TOTAL_IMAGE_BYTES,
  STORED_IMAGE_TYPES,
} from "../../model"
import { BUSY_RETRY_DELAYS_MS, gatewayEffects } from "./effects"

/** A backoff nothing in the test should reach: waiting here is the failure. */
const unexpectedWait = () => Promise.reject(new Error("no wait expected"))
const effectsOf = (client: () => NessaClient | null) =>
  gatewayEffects(client, unexpectedWait)
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
  const effects = effectsOf(
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
  const effects = effectsOf(() => ({ conversation: { read } }) as unknown as NessaClient)
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
/** A signal nobody aborts. */
const live = () => new AbortController().signal
const owed = { requestId: "r", state: "upload_required", ticket, expiresAtMs: 1 }
function staging(
  attachments: { begin: unknown; upload?: unknown },
  wait: (ms: number) => Promise<void> = unexpectedWait,
) {
  return gatewayEffects(
    () =>
      ({ attachments: { upload: vi.fn(), ...attachments } }) as unknown as NessaClient,
    wait,
  )
}

it("holds every message bound the model keeps to the one the protocol generated", () => {
  // Two sets of constants on purpose — the conversation model does not import a
  // client SDK — and this adapter, which sees both, is where they are held to
  // each other. The client's side of each is generated from
  // `protocol/product/v1.json`, so a schema change that nobody carried into the
  // model fails here rather than at the gateway.
  expect([...STORED_IMAGE_TYPES]).toEqual([...IMAGE_ATTACHMENT_TYPES])
  expect(MAX_SEND_IMAGES).toBe(MAX_MESSAGE_IMAGES)
  expect(MAX_SEND_TOTAL_IMAGE_BYTES).toBe(MAX_MESSAGE_IMAGE_BYTES)
  // What one file may weigh to be attached at all is the upload path's bound.
  expect(MAX_ATTACHMENT_BYTES).toBe(MAX_UPLOAD_BYTES)
})

it("forwards a message's images with its text, for send and steer alike", async () => {
  const receipt = { executionId: "execution", requestId: "action", disposition: "queued" }
  const send = vi.fn(async () => receipt)
  const steer = vi.fn(async () => receipt)
  const effects = effectsOf(
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

it.each([
  ["image_input_unsupported", "image-input-unsupported"],
  ["attachment_not_found", "attachment-not-found"],
  ["attachment_unavailable", "attachment-unavailable"],
  ["conversation_not_found", "conversation-not-found"],
  ["conversation_capacity", "conversation-capacity"],
  ["agent_not_configured", "agent-not-configured"],
  ["invalid_request", "invalid-request"],
] as const)(
  "turns the gateway's pre-admission refusal %s into a typed refusal, for send and steer",
  async (code, reason) => {
    const refuse = (conversationId: string) =>
      Promise.reject(
        new NessaConversationMutationError(
          conversationId,
          "action",
          "execution",
          // The message names another code: only the typed one is read.
          new NessaRpcError(code, "temporarily_unavailable"),
          () => Promise.reject(new Error("unused")),
        ),
      )
    const effects = effectsOf(
      () => ({ conversation: { send: refuse, steer: refuse } }) as unknown as NessaClient,
    )
    const submission = {
      conversationId: "server",
      executionId: "execution",
      actionId: "action",
      text: "look",
      attachments: [stored],
    }
    for (const submit of [effects.send, effects.steer]) {
      const error = await submit(submission).catch((error: unknown) => error)
      expect(error).toBeInstanceOf(SubmissionRefusedError)
      expect(error).toMatchObject({ reason })
    }
  },
)

it("reports the client refusing a message's images as a certain refusal, for send and steer", async () => {
  // The client validates a message's images before anything reaches the wire —
  // one boundary — and answers a bad argument with a TypeError. Nothing was
  // sent, so this is as certain as a refusal gets: not "delivery unknown".
  const refuse = () => {
    throw new TypeError(
      `Invalid message attachments: an image must contain 1-${MAX_IMAGE_ATTACHMENT_BYTES} bytes`,
    )
  }
  const effects = effectsOf(
    () => ({ conversation: { send: refuse, steer: refuse } }) as unknown as NessaClient,
  )
  for (const submit of [effects.send, effects.steer]) {
    const error = await submit({
      conversationId: "server",
      executionId: "execution",
      actionId: "action",
      text: "look",
      attachments: [{ ...stored, size: MAX_IMAGE_ATTACHMENT_BYTES + 1 }],
    }).catch((error: unknown) => error)
    expect(error).toBeInstanceOf(SubmissionRefusedError)
    expect(error).toMatchObject({ reason: "invalid-request" })
    // The client's sentence names what is wrong with the message; keep it.
    expect((error as Error).message).toMatch(/an image must contain/)
  }
})

it("passes an uncertain send failure on untouched: a lost answer is not a refusal", async () => {
  const lost = new NessaConversationMutationError(
    "server",
    "action",
    "execution",
    new NessaRpcError("temporarily_unavailable", "attachment_not_found"),
    () => Promise.reject(new Error("unused")),
  )
  const effects = effectsOf(
    () =>
      ({ conversation: { send: () => Promise.reject(lost) } }) as unknown as NessaClient,
  )
  const error = await effects
    .send({
      conversationId: "server",
      executionId: "execution",
      actionId: "action",
      text: "look",
      attachments: [],
    })
    .catch((error: unknown) => error)
  expect(error).toBe(lost)
})

it("uploads nothing when the conversation already holds the bytes, and answers with its reference", async () => {
  const begin = vi.fn(async () => ({ requestId: "r", state: "stored", stored }))
  const upload = vi.fn()
  expect(
    await staging({ begin, upload }).stageAttachment("server", file, bytes, live()),
  ).toEqual(stored)
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
    live(),
  )
  expect(upload).toHaveBeenCalledExactlyOnceWith(
    ticket,
    { mimeType: "image/heic", bytes },
    { signal: expect.any(AbortSignal) },
  )
  // The gateway's reference, not a restatement of what was uploaded.
  expect(reference).toEqual(stored)
  expect(reference.digest).not.toBe(file.digest)
})

it.each([
  ["a file that is not an image", { mimeType: "application/pdf" }, "unsupported-image"],
  [
    "an image the gateway left in an encoding no message names",
    { mimeType: "image/heic" },
    "unsupported-image",
  ],
  // Two different facts, and the tile offers a retry for neither — but it says
  // something different for each, so a readable 6 MiB PNG is not reported as a
  // format the gateway could not read.
  [
    "a readable image over the protocol's image bound",
    { size: MAX_IMAGE_ATTACHMENT_BYTES + 1 },
    "too-large",
  ],
  // Neither fact: the gateway answered something this window cannot use at all.
  ["a reference that is malformed", { digest: "sha256:NOPE" }, "rejected"],
] as const)(
  "does not hand a message %s, however validly it was stored",
  async (_name, change, reason) => {
    const kept = { ...stored, ...change }
    for (const attachments of [
      { begin: async () => ({ requestId: "r", state: "stored", stored: kept }) },
      { begin: async () => owed, upload: async () => kept },
    ])
      await expect(
        staging(attachments).stageAttachment("server", file, bytes, live()),
      ).rejects.toMatchObject({ reason })
  },
)

it("keeps an image at exactly the protocol's bound", async () => {
  const kept = { ...stored, size: MAX_IMAGE_ATTACHMENT_BYTES }
  expect(
    await staging({ begin: async () => owed, upload: async () => kept }).stageAttachment(
      "server",
      file,
      bytes,
      live(),
    ),
  ).toEqual(kept)
})

it.each([
  ["unsupported_image", "unsupported-image"],
  ["image_too_large", "too-large"],
  ["image_input_unsupported", "image-input-unsupported"],
  ["upload_interrupted", "interrupted"],
  ["attachment_not_kept", "interrupted"],
  ["upload_timeout", "interrupted"],
  ["aborted", "interrupted"],
  ["ticket_invalid", "unavailable"],
  ["storage_unavailable", "unavailable"],
  ["audit_unavailable", "unavailable"],
  ["unreachable", "unavailable"],
  ["size_mismatch", "rejected"],
  ["digest_mismatch", "rejected"],
  ["unexpected_response", "rejected"],
] as const)("maps an upload that failed as %s to %s", async (code, reason) => {
  const cause = new NessaAttachmentError(code, 400)
  const upload = vi.fn(() => Promise.reject(cause))
  const error = await staging({ begin: async () => owed, upload })
    .stageAttachment("server", file, bytes, live())
    .catch((error: unknown) => error)
  expect(error).toBeInstanceOf(AttachmentStagingError)
  expect(error).toMatchObject({ reason, cause })
  // Every one of these spends the ticket or leaves it unknown: no second PUT.
  expect(upload).toHaveBeenCalledOnce()
})

/** A backoff the test ends by hand: each wait is a promise it resolves. */
function manualWait() {
  const waits: { ms: number; done: () => void }[] = []
  const asked = { next: () => {} }
  const wait = (ms: number) =>
    new Promise<void>((done) => {
      waits.push({ ms, done })
      asked.next()
    })
  /** Resolves once the adapter is waiting for the `count`th time. */
  const reached = (count: number) =>
    new Promise<void>((resolve) => {
      const check = () => (waits.length >= count ? resolve() : undefined)
      asked.next = check
      check()
    })
  return { wait, waits, reached }
}
const busy = () => new NessaAttachmentError("temporarily_unavailable", 503)

it("offers the same ticket again after a short wait when the gateway had no room", async () => {
  const clock = manualWait()
  const begin = vi.fn(async () => owed)
  const upload = vi
    .fn()
    .mockRejectedValueOnce(busy())
    .mockRejectedValueOnce(busy())
    .mockResolvedValueOnce(stored)
  const staged = staging({ begin, upload }, clock.wait).stageAttachment(
    "server",
    file,
    bytes,
    live(),
  )
  await clock.reached(1)
  // Nothing is sent again until the wait is over.
  expect(upload).toHaveBeenCalledOnce()
  clock.waits[0]!.done()
  await clock.reached(2)
  expect(upload).toHaveBeenCalledTimes(2)
  clock.waits[1]!.done()
  expect(await staged).toEqual(stored)
  expect(clock.waits.map((wait) => wait.ms)).toEqual(BUSY_RETRY_DELAYS_MS.slice(0, 2))
  // One begin, one ticket: that refusal is the one that does not spend it.
  expect(begin).toHaveBeenCalledOnce()
  expect(upload.mock.calls.map(([used]) => used)).toEqual([ticket, ticket, ticket])
})

it("gives up as busy after a bounded number of tries, and says so on the tile's terms", async () => {
  const clock = manualWait()
  const upload = vi.fn(() => Promise.reject(busy()))
  const staged = staging({ begin: async () => owed, upload }, clock.wait)
    .stageAttachment("server", file, bytes, live())
    .catch((error: unknown) => error)
  for (let index = 0; index < BUSY_RETRY_DELAYS_MS.length; index++) {
    await clock.reached(index + 1)
    clock.waits[index]!.done()
  }
  expect(await staged).toMatchObject({ reason: "busy" })
  expect(upload).toHaveBeenCalledTimes(BUSY_RETRY_DELAYS_MS.length + 1)
  expect(clock.waits.map((wait) => wait.ms)).toEqual([...BUSY_RETRY_DELAYS_MS])
})

it("stops offering the ticket when the session went away during the wait", async () => {
  const clock = manualWait()
  const upload = vi.fn(() => Promise.reject(busy()))
  let status = "connected"
  const effects = gatewayEffects(
    () =>
      ({
        connectionState: { status },
        attachments: { begin: async () => owed, upload },
      }) as unknown as NessaClient,
    clock.wait,
  )
  const staged = effects
    .stageAttachment("server", file, bytes, live())
    .catch((error: unknown) => error)
  await clock.reached(1)
  status = "reconnecting"
  clock.waits[0]!.done()
  expect(await staged).toMatchObject({ reason: "unavailable" })
  expect(upload).toHaveBeenCalledOnce()
})

it.each([
  ["attachment_capacity", "busy"],
  ["temporarily_unavailable", "busy"],
  ["attachment_storage_unavailable", "unavailable"],
  ["storage_unavailable", "unavailable"],
  ["audit_unavailable", "unavailable"],
  ["agent_not_configured", "unavailable"],
  ["conversation_not_found", "unavailable"],
  ["invalid_request", "rejected"],
  ["unexpected", "rejected"],
] as const)("maps a begin the gateway refused as %s to %s", async (refusal, reason) => {
  const upload = vi.fn()
  const begin = () =>
    Promise.reject(
      new NessaAttachmentError("begin_refused", undefined, undefined, refusal),
    )
  await expect(
    staging({ begin, upload }).stageAttachment("server", file, bytes, live()),
  ).rejects.toMatchObject({ reason })
  expect(upload).not.toHaveBeenCalled()
})

it("maps an unanswered begin, and one the client would not send, without reaching the upload", async () => {
  const upload = vi.fn()
  for (const [error, reason] of [
    [new NessaAttachmentError("unreachable"), "unavailable"],
    // The client refusing to describe these bytes at all; asking again changes nothing.
    [new TypeError("Media type must be lowercase, without parameters"), "rejected"],
  ] as const) {
    await expect(
      staging({ begin: () => Promise.reject(error), upload }).stageAttachment(
        "server",
        file,
        bytes,
        live(),
      ),
    ).rejects.toMatchObject({ reason })
  }
  expect(upload).not.toHaveBeenCalled()
})

it("reports staging with no live connection as unavailable, asking nothing", async () => {
  const begin = vi.fn()
  const offline = effectsOf(() => null)
  await expect(
    offline.stageAttachment("server", file, bytes, live()),
  ).rejects.toMatchObject({
    reason: "unavailable",
  })
  const reconnecting = effectsOf(
    () =>
      ({
        connectionState: { status: "reconnecting" },
        attachments: { begin },
      }) as unknown as NessaClient,
  )
  await expect(
    reconnecting.stageAttachment("server", file, bytes, live()),
  ).rejects.toBeInstanceOf(AttachmentStagingError)
  expect(begin).not.toHaveBeenCalled()
})

it("hands the caller's signal to the upload, and stops offering a busy ticket once it is aborted", async () => {
  const begin = vi.fn(async () => owed)
  const stopping = new AbortController()
  const upload = vi.fn(async () => {
    // Removed while the gateway had no room: the wait is not worth taking.
    stopping.abort()
    throw busy()
  })
  const error = await staging({ begin, upload }, unexpectedWait)
    .stageAttachment("server", file, bytes, stopping.signal)
    .catch((cause: unknown) => cause)
  expect(upload).toHaveBeenCalledExactlyOnceWith(
    ticket,
    { mimeType: file.mimeType, bytes },
    { signal: stopping.signal },
  )
  expect(error).toBeInstanceOf(AttachmentStagingError)
})
