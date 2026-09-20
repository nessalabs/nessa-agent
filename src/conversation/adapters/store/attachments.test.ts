/**
 * What `sendDraft` does with a draft that holds files, and how a file gets from
 * attached to stored, against the real store and substituted effects.
 *
 * Every refusal here has the same three properties, and each test checks all
 * three: the thunk rejects with a typed `kind`, the conversation shows a reason,
 * and the draft is exactly what it was with nothing sent. Nothing sleeps; an
 * effect that must stay in flight is a promise the test resolves itself.
 */
import { expect, it, vi } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { AttachmentStagingError, type ConversationEffects } from "../../application/ports"
import type { ConversationView } from "../../application/view"
import type { FileAttachment, ImageReference } from "../../model"
import { scenarioEffects } from "../scenario/effects"
import {
  attachFiles,
  controlConversation,
  refreshConversation,
  removeFile,
  sendDraft,
  stageAttachment,
  uploadChanged,
} from "./slice"

/** The digest this window computed over the original bytes. It identifies the upload only. */
const LOCAL_DIGEST = `sha256:${"ab".repeat(32)}`
/** What the gateway stored instead, and so the only thing a message may name. */
const stored: ImageReference = {
  digest: `sha256:${"ef".repeat(32)}`,
  mimeType: "image/jpeg",
  size: 2,
}
const bytes = new Blob(["raw"], { type: "image/heic" })

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}

const image = (id: string, change: Partial<FileAttachment> = {}): FileAttachment => ({
  type: "file",
  id,
  name: `${id}.heic`,
  mimeType: "image/heic",
  size: 3,
  previewUrl: `blob:${id}`,
  upload: { status: "not-started" },
  ...change,
})
const described = (file: FileAttachment) => ({
  digest: LOCAL_DIGEST,
  mimeType: file.mimeType,
  size: file.size,
})

function storeWith(overrides: Partial<ConversationEffects> = {}) {
  const echo = scenarioEffects("echo")
  const effects = {
    ...echo,
    send: vi.fn(echo.send),
    steer: vi.fn(echo.steer),
    // The gateway's answer, whatever was uploaded: a normalized reference.
    stageAttachment: vi.fn<ConversationEffects["stageAttachment"]>(async () => stored),
    ...overrides,
  }
  const store = makeStore(createDependencies({ conversation: effects }))
  const draft = () => store.getState().conversation.conversations[0]!.draft
  const current = () => store.getState().conversation.conversations[0]!
  return { store, effects, draft, current }
}

/** Attach an image and take it all the way to stored, as the panel would. */
async function attachStored(
  store: ReturnType<typeof storeWith>["store"],
  file: FileAttachment,
) {
  store.dispatch(attachFiles({ files: [file], conversationId: "c0" }))
  store.dispatch(uploadChanged({ fileId: file.id, to: "uploading" }))
  await store.dispatch(
    stageAttachment({ id: "c0", fileId: file.id, file: described(file), bytes }),
  )
}

async function expectRefused(
  context: ReturnType<typeof storeWith>,
  kind: string,
  reason: RegExp,
  content: Parameters<typeof sendDraft>[0]["content"] = [{ type: "text", text: "hello" }],
) {
  const before = context.draft()
  const result = await context.store.dispatch(sendDraft({ content, id: "c0" }))
  expect(result.meta.requestStatus).toBe("rejected")
  expect(result.payload).toEqual({ kind })
  expect(context.current().error).toMatch(reason)
  expect(context.draft()).toEqual(before)
  expect(context.current().turns).toEqual([])
  expect(context.effects.send).not.toHaveBeenCalled()
}

it("refuses a file that is not an image, and says which", async () => {
  const context = storeWith()
  const notes = image("notes", { name: "notes.pdf", mimeType: "application/pdf" })
  context.store.dispatch(attachFiles({ files: [notes], conversationId: "c0" }))
  // Whether or not the caller's content mentions the file, the draft holds it.
  for (const content of [
    [{ type: "text" as const, text: "hello" }],
    [{ type: "text" as const, text: "hello" }, notes],
    [notes],
  ])
    await expectRefused(
      context,
      "unsupported-file",
      /notes\.pdf.*cannot be sent/,
      content,
    )
  expect(context.effects.stageAttachment).not.toHaveBeenCalled()
})

it("refuses while an upload is in flight", async () => {
  const context = storeWith()
  context.store.dispatch(attachFiles({ files: [image("a")], conversationId: "c0" }))
  await expectRefused(context, "upload-in-flight", /still uploading/)
  context.store.dispatch(uploadChanged({ fileId: "a", to: "uploading" }))
  await expectRefused(context, "upload-in-flight", /still uploading/)
})

it.each(["rejected", "unavailable", "unsupported-image", "too-large"] as const)(
  "refuses after an upload failed as %s, and points at the tile",
  async (reason) => {
    const context = storeWith()
    context.store.dispatch(attachFiles({ files: [image("a")], conversationId: "c0" }))
    context.store.dispatch(uploadChanged({ fileId: "a", to: "uploading" }))
    context.store.dispatch(uploadChanged({ fileId: "a", to: "failed", reason }))
    await expectRefused(context, "upload-failed", /a\.heic.*did not upload.*tile/)
  },
)

it("refuses more images, or more stored bytes, than one message carries", async () => {
  const many = storeWith()
  for (let index = 0; index < 11; index++)
    await attachStored(many.store, image(`i${index}`))
  await expectRefused(many, "too-many-images", /up to 10 images/)

  // Counted over what the gateway stored: three tiny files stored at 4 MB each.
  const heavy = storeWith({
    stageAttachment: vi.fn(async () => ({ ...stored, size: 4 * 1024 * 1024 })),
  })
  for (let index = 0; index < 3; index++)
    await attachStored(heavy.store, image(`h${index}`))
  await expectRefused(heavy, "images-too-large", /10 MB of images/)
})

it("sends files far over the message total when what the gateway stored fits", async () => {
  const context = storeWith({ read: viewSaying(true) })
  for (let index = 0; index < 2; index++)
    await attachStored(context.store, image(`big${index}`, { size: 18 * 1024 * 1024 }))
  await context.store.dispatch(refreshConversation("c0"))
  const sent = await context.store.dispatch(sendDraft({ content: [], id: "c0" }))
  expect(sent.meta.requestStatus).toBe("fulfilled")
  expect(context.effects.send).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({ attachments: [stored, stored] }),
  )
})

it("refuses a file part the draft does not hold rather than dropping it", async () => {
  const context = storeWith()
  await expectRefused(context, "unknown-attachment", /no longer in this draft/, [
    { type: "text", text: "hello" },
    image("stranger", { upload: { status: "stored", image: stored } }),
  ])
})

function viewSaying(imageInput: boolean): ConversationEffects["read"] {
  return async (conversationId): Promise<ConversationView> => ({
    conversationId,
    revision: `image-input-${imageInput}`,
    messages: [],
    pending: [],
    permissions: [],
    tools: [],
    capabilities: {
      queue: true,
      steer: false,
      resume: false,
      permissions: false,
      imageInput,
    },
    truncated: false,
    queueComplete: true,
  })
}

it("refuses images for an agent that does not take them", async () => {
  const context = storeWith({ read: viewSaying(false) })
  await attachStored(context.store, image("a"))
  await context.store.dispatch(refreshConversation("c0"))
  await expectRefused(context, "image-input-unsupported", /does not take images/)
})

it("does not guess before the gateway has said whether the agent takes images", async () => {
  const gate = deferred<ConversationView>()
  const read = vi.fn<ConversationEffects["read"]>(() => gate.promise)
  const context = storeWith({ read })
  await attachStored(context.store, image("a"))
  expect(context.current().remote).toBeUndefined()
  await expectRefused(context, "image-input-unknown", /Still checking/)
  // The refusal asked; it did not wait. The answer lands when it lands.
  expect(read).toHaveBeenCalledOnce()
  gate.resolve(await viewSaying(true)(context.current().serverConversationId!))
  await gate.promise
  await Promise.resolve()
  expect(context.current().remote?.capabilities.imageInput).toBe(true)
  const sent = await context.store.dispatch(
    sendDraft({ content: [{ type: "text", text: "hello" }], id: "c0" }),
  )
  expect(sent.meta.requestStatus).toBe("fulfilled")
})

it("still refuses a draft with nothing in it, silently", async () => {
  const context = storeWith()
  const result = await context.store.dispatch(sendDraft({ content: [], id: "c0" }))
  expect(result.payload).toEqual({ kind: "empty-draft" })
  expect(context.current().error).toBeUndefined()
})

async function readyToSend(overrides: Partial<ConversationEffects> = {}) {
  const context = storeWith({ read: viewSaying(true), ...overrides })
  await attachStored(context.store, image("finder"))
  await context.store.dispatch(refreshConversation("c0"))
  return context
}

it("sends the reference the gateway returned, never the digest this window computed", async () => {
  const context = await readyToSend()
  const result = await context.store.dispatch(
    sendDraft({ content: [{ type: "text", text: "what is this?" }], id: "c0" }),
  )
  expect(result.meta.requestStatus).toBe("fulfilled")
  expect(context.effects.send).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({ text: "what is this?", attachments: [stored] }),
  )
  expect(JSON.stringify(vi.mocked(context.effects.send).mock.calls)).not.toContain(
    LOCAL_DIGEST,
  )
  expect(context.draft()).toEqual([])
})

it("sends a message of images alone, and titles the tab by the image", async () => {
  const context = await readyToSend()
  const result = await context.store.dispatch(sendDraft({ content: [], id: "c0" }))
  expect(result.meta.requestStatus).toBe("fulfilled")
  expect(context.effects.send).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({ text: "", attachments: [stored] }),
  )
  expect(context.current().title).toBe("finder.heic")
})

it("steers with images the same way", async () => {
  const context = await readyToSend()
  await context.store.dispatch(sendDraft({ content: [], id: "c0", steering: true }))
  expect(context.effects.steer).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({ attachments: [stored] }),
  )
  expect(context.effects.send).not.toHaveBeenCalled()
})

it("retries an uncertain send with the identical images and identities", async () => {
  const echo = scenarioEffects("echo")
  const send = vi
    .fn<ConversationEffects["send"]>()
    .mockRejectedValueOnce(new Error("connection lost"))
    .mockImplementation(echo.send)
  const context = await readyToSend({ create: echo.create, send })
  await context.store
    .dispatch(sendDraft({ content: [{ type: "text", text: "look" }], id: "c0" }))
    .unwrap()
    .catch(() => undefined)
  const turn = context.current().turns[0]!
  expect(turn).toMatchObject({ from: "user", receipt: "unknown" })
  await context.store.dispatch(
    controlConversation({
      id: "c0",
      control: { kind: "retry", executionId: turn.executionId! },
    }),
  )
  expect(send).toHaveBeenCalledTimes(2)
  expect(send.mock.calls[1]![0]).toEqual(send.mock.calls[0]![0])
  expect(send.mock.calls[1]![0].attachments).toEqual([stored])
})

it("returns a refused message's images to the draft, still stored and still sendable", async () => {
  const { NessaConversationMutationError, NessaRpcError } = await import("@nessa/client")
  const echo = scenarioEffects("echo")
  const send = vi
    .fn<ConversationEffects["send"]>()
    .mockImplementationOnce((input) =>
      Promise.reject(
        new NessaConversationMutationError(
          input.conversationId,
          input.actionId,
          input.executionId,
          new NessaRpcError("invalid_request", "image input is not available"),
          () => Promise.reject(new Error("unused")),
        ),
      ),
    )
    .mockImplementation(echo.send)
  const context = await readyToSend({ create: echo.create, send })
  const before = context.draft()
  await context.store
    .dispatch(sendDraft({ content: [], id: "c0" }))
    .unwrap()
    .catch(() => undefined)
  expect(context.draft()).toEqual(before)
  expect(context.current().turns[0]).toMatchObject({ receipt: "failed" })
  const again = await context.store.dispatch(sendDraft({ content: [], id: "c0" }))
  expect(again.meta.requestStatus).toBe("fulfilled")
  expect(send.mock.calls[1]![0].attachments).toEqual([stored])
})

it("stages the original into a conversation it creates first, and records what came back", async () => {
  const order: string[] = []
  const echo = scenarioEffects("echo")
  const context = storeWith({
    create: vi.fn(async (id: string) => {
      order.push("create")
      return echo.create(id)
    }),
    stageAttachment: vi.fn(async () => {
      order.push("stage")
      return stored
    }),
  })
  const file = image("a")
  await attachStored(context.store, file)
  expect(order).toEqual(["create", "stage"])
  expect(context.effects.stageAttachment).toHaveBeenCalledExactlyOnceWith(
    context.current().serverConversationId,
    described(file),
    bytes,
  )
  expect(context.current().serverReady).toBe(true)
  // The file still describes the original; the reference is the gateway's.
  expect(context.draft()).toEqual([
    image("a", { upload: { status: "stored", image: stored } }),
  ])
})

it.each([
  ["the gateway refusing the bytes", new AttachmentStagingError("rejected"), "rejected"],
  ["storage being away", new AttachmentStagingError("unavailable"), "unavailable"],
  [
    "a format the gateway cannot read",
    new AttachmentStagingError("unsupported-image"),
    "unsupported-image",
  ],
  [
    "an image that cannot fit the model",
    new AttachmentStagingError("too-large"),
    "too-large",
  ],
  // Not a staging error at all: nothing says the gateway looked at the bytes.
  ["an unrelated fault", new Error("rejected"), "unavailable"],
] as const)("records %s on the tile", async (_name, error, reason) => {
  const context = storeWith({ stageAttachment: vi.fn(() => Promise.reject(error)) })
  await attachStored(context.store, image("a"))
  expect(context.draft()).toEqual([image("a", { upload: { status: "failed", reason } })])
})

it("fails the upload rather than store a reference that is not one", async () => {
  const context = storeWith({
    stageAttachment: vi.fn(async () => ({
      ...stored,
      mimeType: "image/heic" as "image/jpeg",
    })),
  })
  await attachStored(context.store, image("a"))
  expect(context.draft()).toEqual([
    image("a", { upload: { status: "failed", reason: "rejected" } }),
  ])
})

it("fails the upload as unavailable when the conversation cannot be created", async () => {
  const context = storeWith({ create: vi.fn(() => Promise.reject(new Error("offline"))) })
  await attachStored(context.store, image("a"))
  expect(context.effects.stageAttachment).not.toHaveBeenCalled()
  expect(context.draft()).toEqual([
    image("a", { upload: { status: "failed", reason: "unavailable" } }),
  ])
})

it("does not bring back a tile removed while its upload was in flight", async () => {
  const gate = deferred<ImageReference>()
  const started = deferred<void>()
  const context = storeWith({
    stageAttachment: vi.fn(() => {
      started.resolve()
      return gate.promise
    }),
  })
  const file = image("a")
  context.store.dispatch(attachFiles({ files: [file], conversationId: "c0" }))
  context.store.dispatch(uploadChanged({ fileId: "a", to: "uploading" }))
  const staging = context.store.dispatch(
    stageAttachment({ id: "c0", fileId: "a", file: described(file), bytes }),
  )
  // Create has settled and the upload is genuinely in flight before the removal.
  await started.promise
  context.store.dispatch(removeFile("a"))
  expect(context.draft()).toEqual([])
  gate.resolve(stored)
  await staging
  expect(context.draft()).toEqual([])

  // The same, for an upload that fails late.
  const failing = deferred<ImageReference>()
  const begun = deferred<void>()
  const second = storeWith({
    stageAttachment: vi.fn(() => {
      begun.resolve()
      return failing.promise
    }),
  })
  const other = image("b")
  second.store.dispatch(attachFiles({ files: [other], conversationId: "c0" }))
  second.store.dispatch(uploadChanged({ fileId: "b", to: "uploading" }))
  const late = second.store.dispatch(
    stageAttachment({ id: "c0", fileId: "b", file: described(other), bytes }),
  )
  await begun.promise
  second.store.dispatch(removeFile("b"))
  failing.reject(new AttachmentStagingError("unavailable"))
  await late
  expect(second.draft()).toEqual([])
})

it("uploads nothing for a file that is already gone", async () => {
  const context = storeWith()
  await context.store.dispatch(
    stageAttachment({
      id: "c0",
      fileId: "never-attached",
      file: described(image("x")),
      bytes,
    }),
  )
  expect(context.effects.stageAttachment).not.toHaveBeenCalled()
  expect(context.current().serverConversationId).toBeUndefined()
})
