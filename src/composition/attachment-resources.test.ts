import { afterEach, expect, it, vi } from "vitest"
import { createDependencies } from "./dependencies"
import { makeStore } from "../store"
import { MAX_SESSION_ATTACHMENT_BYTES } from "../panel/adapters/attachment-resources"
import {
  attachFiles,
  closeConversation,
  openConversation,
  refreshConversation,
  removeFile,
  scenarioEffects,
  sendDraft,
  setActive,
  stageAttachment,
  uploadChanged,
} from "../conversation/testing"

afterEach(() => vi.restoreAllMocks())

it("releases removed, closed and rejected resources but keeps inactive-tab previews", () => {
  let index = 0
  vi.spyOn(URL, "createObjectURL").mockImplementation(() => `blob:${++index}`)
  const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const dependencies = createDependencies()
  const store = makeStore(dependencies)
  const [a, b] = dependencies.attachments.add([
    new File(["a"], "a.txt"),
    new File(["b"], "b.txt"),
  ])
  store.dispatch(attachFiles({ files: [a!, b!], conversationId: "c0" }))
  store.dispatch(openConversation())
  expect(revoke).not.toHaveBeenCalled()
  store.dispatch(setActive("c0"))
  store.dispatch(removeFile(a!.id))
  expect(revoke).toHaveBeenCalledExactlyOnceWith("blob:1")
  store.dispatch(closeConversation("c0"))
  expect(revoke).toHaveBeenLastCalledWith("blob:2")
  const rejected = dependencies.attachments.add([new File(["x"], "x.txt")])
  store.dispatch(attachFiles({ files: rejected, conversationId: "closed" }))
  expect(revoke).toHaveBeenLastCalledWith("blob:3")
  expect(revoke).toHaveBeenCalledTimes(3)
})

it("keeps a sent image's preview for its turn, and releases it with the conversation", async () => {
  vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:sent")
  const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const dependencies = createDependencies({ conversation: scenarioEffects("echo") })
  const store = makeStore(dependencies)
  const [image] = dependencies.attachments.add([
    new File(["png"], "finder.png", { type: "image/png" }),
  ])
  store.dispatch(attachFiles({ files: [image!], conversationId: "c0" }))
  store.dispatch(uploadChanged({ fileId: image!.id, to: "uploading" }))
  await store.dispatch(
    stageAttachment({
      id: "c0",
      fileId: image!.id,
      file: { digest: `sha256:${"ab".repeat(32)}`, mimeType: "image/png", size: 3 },
      bytes: dependencies.attachments.bytes(image!.id)!,
    }),
  )
  await store.dispatch(refreshConversation("c0"))
  const sent = await store.dispatch(sendDraft({ content: [], id: "c0" }))
  expect(sent.meta.requestStatus).toBe("fulfilled")
  // The draft is empty, the gateway has echoed the turn back by reference, and
  // the transcript still paints the turn from the local URL.
  const conversation = store.getState().conversation.conversations[0]!
  expect(conversation.draft).toEqual([])
  expect(conversation.turns[0]).toMatchObject({
    from: "user",
    receipt: "delivered",
    content: [{ type: "file", previewUrl: "blob:sent" }],
  })
  expect(revoke).not.toHaveBeenCalled()
  expect(dependencies.attachments.bytes(image!.id)).toBeDefined()
  store.dispatch(closeConversation("c0"))
  expect(revoke).toHaveBeenCalledExactlyOnceWith("blob:sent")
})

it("does not let sent messages use up what attaching needs", async () => {
  let index = 0
  vi.spyOn(URL, "createObjectURL").mockImplementation(() => `blob:${++index}`)
  const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const dependencies = createDependencies({ conversation: scenarioEffects("echo") })
  const store = makeStore(dependencies)
  // Five messages, each one 64 MiB original that the gateway stored small.
  // Metadata-only fixtures avoid allocating 320 MiB to test accounting.
  const sentIds: string[] = []
  for (let message = 0; message < 5; message++) {
    const [image] = dependencies.attachments.add([
      { name: `${message}.png`, type: "image/png", size: 64 * 1024 * 1024 } as File,
    ])
    sentIds.push(image!.id)
    store.dispatch(attachFiles({ files: [image!], conversationId: "c0" }))
    store.dispatch(uploadChanged({ fileId: image!.id, to: "uploading" }))
    await store.dispatch(
      stageAttachment({
        id: "c0",
        fileId: image!.id,
        file: { digest: `sha256:${"ab".repeat(32)}`, mimeType: "image/png", size: 3 },
        bytes: new Blob(["png"]),
      }),
    )
    await store.dispatch(refreshConversation("c0"))
    const sent = await store.dispatch(sendDraft({ content: [], id: "c0" }))
    expect(sent.meta.requestStatus).toBe("fulfilled")
  }
  // Every draft is empty, and the whole budget is there to attach with.
  expect(store.getState().conversation.conversations[0]!.draft).toEqual([])
  expect(dependencies.attachments.canAdd(MAX_SESSION_ATTACHMENT_BYTES)).toBe(true)
  // Only the newest original is still kept as a preview; the older four were
  // released and their turns fell back to the reference tile.
  expect(revoke).toHaveBeenCalledTimes(4)
  expect(sentIds.map((id) => dependencies.attachments.bytes(id) !== undefined)).toEqual([
    false,
    false,
    false,
    false,
    true,
  ])
  const kinds = store
    .getState()
    .conversation.conversations[0]!.turns.flatMap((turn) =>
      turn.from === "user" ? turn.content.map((part) => part.type) : [],
    )
  expect(kinds).toEqual([
    "image-reference",
    "image-reference",
    "image-reference",
    "image-reference",
    "file",
  ])
})
