import { afterEach, expect, it, vi } from "vitest"
import { createDependencies } from "./dependencies"
import { makeStore } from "../store"
import { scenarioEffects } from "../conversation/adapters/scenario/effects"
import {
  attachFiles,
  closeConversation,
  openConversation,
  refreshConversation,
  removeFile,
  sendDraft,
  setActive,
  stageAttachment,
  uploadChanged,
} from "../conversation/adapters/store/slice"

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
