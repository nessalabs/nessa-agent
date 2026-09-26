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
import {
  AttachmentStagingError,
  SubmissionRefusedError,
  type ConversationEffects,
} from "../../application/ports"
import { conversationTabSnapshot } from "../../application/saved-tabs"
import type { ConversationView } from "../../application/view"
import type { CommandFailure, FileAttachment, ImageReference } from "../../model"
import { scenarioEffects } from "../scenario/effects"
import {
  attachFiles,
  closeTab,
  controlConversation,
  refreshConversation,
  removeFile,
  sendDraft,
  stageAttachment,
  stopGenerating,
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
  path: null,
  ...change,
})
const described = (file: FileAttachment) => ({
  digest: LOCAL_DIGEST,
  mimeType: file.mimeType,
  size: file.size,
})

/**
 * `canChoosePaths` is the surface fact composition injects: whether there is a
 * picker that can say where a file is. It defaults to the app's answer, which
 * is what every test here but one is about.
 */
function storeWith(overrides: Partial<ConversationEffects> = {}, canChoosePaths = true) {
  const echo = scenarioEffects("echo")
  const effects = {
    ...echo,
    send: vi.fn(echo.send),
    steer: vi.fn(echo.steer),
    // The gateway's answer, whatever was uploaded: a normalized reference.
    stageAttachment: vi.fn<ConversationEffects["stageAttachment"]>(async () => stored),
    ...overrides,
  }
  const store = makeStore(createDependencies({ conversation: effects, canChoosePaths }))
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

// Which drafts cannot go is `declineReason`'s, and tested there over every
// reason. What the thunk does with one of those answers is this file's: the
// reason is shown, the draft is untouched, and nothing is sent.
it("refuses a file that is not an image, and says which", async () => {
  const context = storeWith()
  const notes = image("notes", { name: "notes.pdf", mimeType: "application/pdf" })
  context.store.dispatch(attachFiles({ files: [notes], conversationId: "c0" }))
  // Whether or not the caller's content mentions the file, the draft holds it.
  for (const content of [
    [{ type: "text" as const, text: "hello" }],
    [{ type: "text" as const, text: "hello" }, notes],
  ])
    await expectRefused(
      context,
      "unsupported-file",
      /notes\.pdf.*cannot be sent/,
      content,
    )
  expect(context.effects.stageAttachment).not.toHaveBeenCalled()
})

// The refusal and the notice over the composer are two surfaces saying one
// thing, so they have to agree. The notice already turns on whether there is a
// picker that can say where a file is; this proves the refused send is told
// from the same fact, and that the fact reaches the thunk from composition
// rather than being assumed here — the conversation vertical is not allowed to
// ask the host, and a default of "yes" would send a browser to its own file
// input, which hands back another file with no path and the same refusal.
it("does not send a browser to a picker that would refuse the file again", async () => {
  for (const [canChoosePaths, sentence] of [
    [true, /choose it with \+/],
    [false, /Nessa app/],
  ] as const) {
    const context = storeWith({}, canChoosePaths)
    const notes = image("notes", { name: "notes.pdf", mimeType: "application/pdf" })
    context.store.dispatch(attachFiles({ files: [notes], conversationId: "c0" }))
    await expectRefused(context, "unsupported-file", sentence)
    // Neither surface ever offers what the other one rules out.
    expect(context.current().error).not.toMatch(canChoosePaths ? /Nessa app/ : /\+/)
  }
})

it("refuses more stored bytes than one message carries, counted over what came back", async () => {
  // Three tiny files the gateway stored at 4 MB each: the files are nothing.
  const heavy = storeWith({
    stageAttachment: vi.fn(async () => ({ ...stored, size: 4 * 1024 * 1024 })),
  })
  for (let index = 0; index < 3; index++)
    await attachStored(heavy.store, image(`h${index}`))
  await expectRefused(heavy, "images-too-large", /10 MiB of images/)
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
    title: null,
    revision: `image-input-${imageInput}`,
    messages: [],
    pending: [],
    permissions: [],
    questions: [],
    tools: [],
    capabilities: {
      queue: true,
      steer: false,
      resume: false,
      permissions: false,
      imageInput,
      agentFeatures: {
        permissionDenial: "unknown",
        nativeHookSuppression: "unknown",
        compactionReporting: "unsupported_not_implemented",
        modelSwitchReporting: "unsupported_not_implemented",
        permissionDeferral: "unsupported_not_implemented",
        elicitationForwarding: "unknown",
        preToolPolicy: "unsupported_not_implemented",
        policyEndTurn: "unsupported_not_implemented",
        policyCloseSession: "unsupported_not_implemented",
        incomingElicitation: "unsupported_not_implemented",
      },
    },
    lifecycle: { phase: "attached" },
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

it("sends a message of images alone, leaving the tab's name to the gateway", async () => {
  const context = await readyToSend()
  const result = await context.store.dispatch(sendDraft({ content: [], id: "c0" }))
  expect(result.meta.requestStatus).toBe("fulfilled")
  expect(context.effects.send).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({ text: "", attachments: [stored] }),
  )
  // The gateway names the conversation from its first message; the view brings it.
  expect(context.current().title).toBe("New chat")
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

/** The gateway refusing a send before admitting it, as the effects adapter reports it. */
const refusing =
  (reason: CommandFailure): ConversationEffects["send"] =>
  () =>
    Promise.reject(new SubmissionRefusedError(reason))

it.each([
  ["image-input-unsupported", /does not take images/],
  ["conversation-capacity", /too many conversations/],
  ["conversation-not-found", /no longer has this conversation/],
  // The word the client used to report as uncertain, so this sentence and the
  // notice above the composer were both unreachable.
  ["conversation-state-unreadable", /cannot read this conversation's saved state/],
] as const)(
  "knows a message refused as %s was not sent: says why, and brings it back still stored",
  async (reason, text) => {
    const echo = scenarioEffects("echo")
    const send = vi
      .fn<ConversationEffects["send"]>()
      .mockImplementationOnce(refusing(reason))
      .mockImplementation(echo.send)
    const context = await readyToSend({ create: echo.create, send })
    const before = context.draft()
    await context.store
      .dispatch(sendDraft({ content: [{ type: "text", text: "look" }], id: "c0" }))
      .unwrap()
      .catch(() => undefined)
    // Not "delivery unknown": the gateway said no, so the draft is back.
    expect(context.current().turns[0]).toMatchObject({ receipt: "failed" })
    expect(context.current().error).toMatch(text)
    expect(context.draft()).toEqual([{ type: "text", text: "look" }, ...before])
    const again = await context.store.dispatch(
      sendDraft({ content: [{ type: "text", text: "look" }], id: "c0" }),
    )
    expect(again.meta.requestStatus).toBe("fulfilled")
    expect(send.mock.calls[1]![0].attachments).toEqual([stored])
  },
)

it.each(["attachment-not-found", "attachment-unavailable"] as const)(
  "brings a message refused as %s back with its images to upload again, not the dead reference",
  async (reason) => {
    const echo = scenarioEffects("echo")
    const context = await readyToSend({
      create: echo.create,
      send: vi.fn(refusing(reason)),
    })
    const file = context.draft()[0]!
    await context.store
      .dispatch(sendDraft({ content: [], id: "c0" }))
      .unwrap()
      .catch(() => undefined)
    expect(context.current().turns[0]).toMatchObject({ receipt: "failed" })
    expect(context.current().error).toMatch(/uploading again/)
    // The same file, no longer claiming to be stored: the panel uploads what is
    // not started, and sending before it has is refused with its own reason.
    expect(context.draft()).toEqual([{ ...file, upload: { status: "not-started" } }])
    const early = await context.store.dispatch(sendDraft({ content: [], id: "c0" }))
    expect(early.payload).toEqual({ kind: "upload-in-flight" })
  },
)

it("knows a message the client would not put on the wire was never sent", async () => {
  // The model puts no per-image byte bound on a stored reference: how heavy one
  // image may be is the protocol's rule, enforced by the client, which refuses
  // the arguments before anything is sent. A refusal there is a refusal, not a
  // lost acknowledgement, so the turn is failed and the draft comes back rather
  // than the panel showing a message of unknown delivery with nothing to edit.
  const context = await readyToSend({
    stageAttachment: vi.fn(async () => ({ ...stored, size: 6 * 1024 * 1024 })),
  })
  const before = context.draft()
  await context.store
    .dispatch(sendDraft({ content: [{ type: "text", text: "look" }], id: "c0" }))
    .unwrap()
    .catch(() => undefined)
  // "failed", not "unknown": nothing was sent, so delivery is not in question.
  expect(context.current().turns[0]).toMatchObject({ receipt: "failed" })
  expect(context.current().error).toMatch(/an image must contain/)
  expect(context.draft()).toEqual([{ type: "text", text: "look" }, ...before])
})

it("forgets a draft's stored images when Stop closes the conversation that held them", async () => {
  // The trigger: attach while the agent runs, the upload finishes, press Stop.
  // The gateway releases every hold on close, so `stored` would now be a lie
  // that the next send could only have refused as attachment_not_found.
  const close = vi.fn(async () => {})
  const context = await readyToSend({ close })
  const file = context.draft()[0]!
  expect(file).toMatchObject({ upload: { status: "stored" } })
  await context.store.dispatch(stopGenerating({ conversationId: "c0" }))
  expect(close).toHaveBeenCalledOnce()
  expect(context.draft()).toEqual([{ ...file, upload: { status: "not-started" } }])
  // Uploaded again, as the panel would, it sends.
  context.store.dispatch(
    uploadChanged({ fileId: file.type === "file" ? file.id : "", to: "uploading" }),
  )
  await context.store.dispatch(
    stageAttachment({
      id: "c0",
      fileId: file.type === "file" ? file.id : "",
      file: described(file as FileAttachment),
      bytes,
    }),
  )
  const sent = await context.store.dispatch(sendDraft({ content: [], id: "c0" }))
  expect(sent.meta.requestStatus).toBe("fulfilled")
})

it("forgets them too when the close went unanswered: it may have applied", async () => {
  const context = await readyToSend({
    close: vi.fn(() => Promise.reject(new Error("connection lost"))),
  })
  await context.store
    .dispatch(stopGenerating({ conversationId: "c0" }))
    .unwrap()
    .catch(() => undefined)
  expect(context.draft()[0]).toMatchObject({ upload: { status: "not-started" } })
})

it("declines a send with no session yet, with a reason and the draft kept", async () => {
  const context = await readyToSend()
  const before = context.draft()
  const result = await context.store.dispatch(
    sendDraft({ content: [{ type: "text", text: "hello" }], id: "c0", connected: false }),
  )
  expect(result.payload).toEqual({ kind: "not-connected" })
  expect(context.current().error).toMatch(
    /Not connected to the gateway yet.*draft has been kept/,
  )
  expect(context.draft()).toEqual(before)
  expect(context.current().turns).toEqual([])
  expect(context.effects.send).not.toHaveBeenCalled()
})

it("closes on the gateway a conversation it only ever uploaded into, when its tab closes", async () => {
  const close = vi.fn(async () => {})
  const context = await readyToSend({ close })
  const serverId = context.current().serverConversationId
  await context.store.dispatch(closeTab("c0"))
  expect(close).toHaveBeenCalledExactlyOnceWith(serverId)
  expect(
    context.store.getState().conversation.conversations.some((item) => item.id === "c0"),
  ).toBe(false)
})

it("closes the tab and nothing else when the conversation has turns or has not been read", async () => {
  // Somebody's work: closing a tab never stops it.
  const close = vi.fn(async () => {})
  const spoken = await readyToSend({ close })
  await spoken.store.dispatch(sendDraft({ content: [], id: "c0" }))
  await spoken.store.dispatch(closeTab("c0"))
  // No view yet: it may be a restored conversation with a history.
  const gate = deferred<ConversationView>()
  const unread = storeWith({ close, read: () => gate.promise })
  await attachStored(unread.store, image("a"))
  await unread.store.dispatch(closeTab("c0"))
  expect(close).not.toHaveBeenCalled()
})

it("still closes the tab when the gateway will not close the conversation", async () => {
  const warned = vi.spyOn(console, "warn").mockImplementation(() => {})
  const context = await readyToSend({
    close: vi.fn(() => Promise.reject(new Error("offline"))),
  })
  await context.store.dispatch(closeTab("c0"))
  expect(
    context.store.getState().conversation.conversations.some((item) => item.id === "c0"),
  ).toBe(false)
  expect(warned).toHaveBeenCalledOnce()
  warned.mockRestore()
})

it("does not save a tab for a conversation known to be empty, and does once it is not", async () => {
  const context = await readyToSend()
  const file = context.draft()[0] as FileAttachment
  const saved = () =>
    conversationTabSnapshot(context.store.getState().conversation).tabs.length
  // Bound to a gateway conversation by the upload, with an image in the draft.
  expect(saved()).toBe(1)
  context.store.dispatch(removeFile(file.id))
  // Attach-then-remove: read, empty, nothing drafted. Not worth coming back to.
  expect(saved()).toBe(0)
  await attachStored(context.store, image("again"))
  await context.store.dispatch(sendDraft({ content: [], id: "c0" }))
  expect(saved()).toBe(1)
})

it("keeps a tab whose conversation has not been read yet: it may have a history", async () => {
  const gate = deferred<ConversationView>()
  const context = storeWith({ read: () => gate.promise })
  await attachStored(context.store, image("a"))
  context.store.dispatch(removeFile("a"))
  expect(
    conversationTabSnapshot(context.store.getState().conversation).tabs,
  ).toHaveLength(1)
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
    expect.any(AbortSignal),
  )
  expect(context.current().serverReady).toBe(true)
  // The file still describes the original; the reference is the gateway's.
  expect(context.draft()).toEqual([
    image("a", { upload: { status: "stored", image: stored } }),
  ])
})

// Which reason a staging refusal carries is the gateway adapter's, and tested
// there. Here: a typed refusal reaches the tile as itself, and anything else —
// nothing says the gateway ever looked at the bytes — is the gateway being away.
it.each([
  ["the gateway refusing the bytes", new AttachmentStagingError("rejected"), "rejected"],
  ["an unrelated fault", new Error("rejected"), "unavailable"],
] as const)("records %s on the tile", async (_name, error, reason) => {
  const context = storeWith({ stageAttachment: vi.fn(() => Promise.reject(error)) })
  await attachStored(context.store, image("a"))
  expect(context.draft()).toEqual([image("a", { upload: { status: "failed", reason } })])
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
  // A late failure is the same: `changeUpload` ignores every result for a file
  // the draft no longer holds, whichever way the upload ended.
  expect(context.draft()).toEqual([])
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

it("says an image cannot go into a conversation deleted elsewhere, and offers no retry", async () => {
  // The gateway refuses to open a deleted conversation before any upload
  // begins; that is a deletion, not a gateway that could not be reached.
  const context = storeWith({
    create: async () => {
      throw new SubmissionRefusedError("conversation-deleted")
    },
  })
  const file = image("a")
  await attachStored(context.store, file)
  expect(context.draft()[0]).toMatchObject({
    upload: { status: "failed", reason: "conversation-deleted" },
  })
})
