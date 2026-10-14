import { createNextState } from "@reduxjs/toolkit"
import { attachmentResourceUsage } from "../conversation/adapters/store/slice"
import { afterEach, expect, it, vi } from "vitest"
import { createDependencies } from "./dependencies"
import { makeStore } from "../store"
import { MAX_SESSION_ATTACHMENT_BYTES } from "../panel/adapters/attachment-resources"
import {
  AttachmentResource,
  SubmissionRefusedError,
  attachFiles,
  closeConversation,
  openConversation,
  followConversation,
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
  store.dispatch(removeFile("a1"))
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
  store.dispatch(
    uploadChanged({ conversationId: "c0", placementId: "a1", to: "uploading" }),
  )
  await store.dispatch(
    stageAttachment({
      id: "c0",
      placementId: "a1",
      file: { digest: `sha256:${"ab".repeat(32)}`, mimeType: "image/png", size: 3 },
      bytes: dependencies.attachments.bytes(image!)!,
    }),
  )
  await store.dispatch(followConversation("c0"))
  const sent = await store.dispatch(sendDraft({ content: [], id: "c0" }))
  expect(sent.meta.requestStatus).toBe("fulfilled")
  // The draft is empty, the gateway has echoed the turn back by reference, and
  // the transcript still paints the turn from the local URL.
  const conversation = store.getState().conversation.conversations[0]!
  expect(conversation.draft).toEqual([])
  expect(conversation.turns[0]).toMatchObject({
    from: "user",
    receipt: "delivered",
    content: [{ type: "file", resource: { previewUrl: "blob:sent" } }],
  })
  expect(revoke).not.toHaveBeenCalled()
  expect(dependencies.attachments.bytes(image!)).toBeDefined()
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
  // Native Blob sizes own accounting; accepted previews release older originals.
  const sentIds: import("../conversation/testing").AttachmentResource[] = []
  for (let message = 0; message < 5; message++) {
    const [image] = dependencies.attachments.add([
      new File([new ArrayBuffer(64 * 1024 * 1024)], `${message}.png`, {
        type: "image/png",
      }),
    ])
    sentIds.push(image!)
    store.dispatch(attachFiles({ files: [image!], conversationId: "c0" }))
    store.dispatch(
      uploadChanged({
        conversationId: "c0",
        placementId: `a${message + 1}`,
        to: "uploading",
      }),
    )
    await store.dispatch(
      stageAttachment({
        id: "c0",
        placementId: `a${message + 1}`,
        file: { digest: `sha256:${"ab".repeat(32)}`, mimeType: "image/png", size: 3 },
        bytes: new Blob(["png"]),
      }),
    )
    await store.dispatch(followConversation("c0"))
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

it("AL7/AL9: shared resources stay budgeted until the last needed placement is gone", async () => {
  vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:shared")
  const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const dependencies = createDependencies({ conversation: scenarioEffects("echo") })
  const store = makeStore(dependencies)
  const [resource] = dependencies.attachments.add([
    new File(["png"], "shared.png", { type: "image/png" }),
  ])
  store.dispatch(attachFiles({ files: [resource!], conversationId: "c0" }))
  store.dispatch(openConversation())
  store.dispatch(attachFiles({ files: [resource!], conversationId: "c1" }))
  expect(dependencies.attachments.canAdd(MAX_SESSION_ATTACHMENT_BYTES - 3)).toBe(true)
  expect(dependencies.attachments.canAdd(MAX_SESSION_ATTACHMENT_BYTES - 2)).toBe(false)
  store.dispatch(
    uploadChanged({ conversationId: "c0", placementId: "a1", to: "uploading" }),
  )
  await store.dispatch(
    stageAttachment({
      id: "c0",
      placementId: "a1",
      file: { digest: `sha256:${"ab".repeat(32)}`, mimeType: "image/png", size: 3 },
      bytes: dependencies.attachments.bytes(resource!)!,
    }),
  )
  await store.dispatch(followConversation("c0"))
  const sent = await store.dispatch(sendDraft({ content: [], id: "c0" }))
  expect(sent.meta.requestStatus).toBe("fulfilled")
  // One accepted preview does not exempt a second draft sharing the original.
  expect(dependencies.attachments.canAdd(MAX_SESSION_ATTACHMENT_BYTES - 2)).toBe(false)
  store.dispatch(closeConversation("c1"))
  expect(dependencies.attachments.canAdd(MAX_SESSION_ATTACHMENT_BYTES)).toBe(true)
  expect(dependencies.attachments.bytes(resource!)).toBeDefined()
  expect(revoke).not.toHaveBeenCalled()
  store.dispatch(closeConversation("c0"))
  expect(dependencies.attachments.bytes(resource!)).toBeUndefined()
  expect(revoke).toHaveBeenCalledExactlyOnceWith("blob:shared")
})

it("AL17/AL19/AL20: fabricated size-one metadata cannot hide 128MiB native originals from preview accounting", async () => {
  let index = 0
  vi.spyOn(URL, "createObjectURL").mockImplementation(() => `blob:native-${++index}`)
  vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const gateway = scenarioEffects("echo")
  const dependencies = createDependencies({
    conversation: {
      ...gateway,
      // The gateway normalizes originals; its retained reference is smaller.
      stageAttachment: (id, _file, _bytes, signal) =>
        gateway.stageAttachment(
          id,
          {
            digest: `sha256:${"ef".repeat(32)}`,
            mimeType: "image/png",
            size: 1,
          },
          new Blob(["x"], { type: "image/png" }),
          signal,
        ),
    },
  })
  const store = makeStore(dependencies)
  store.dispatch(openConversation())
  const size = 64 * 1024 * 1024
  const originals = dependencies.attachments.add([
    new File([new ArrayBuffer(size)], "one.png", { type: "image/png" }),
    new File([new ArrayBuffer(size)], "two.png", { type: "image/png" }),
  ])
  store.dispatch(attachFiles({ conversationId: "c1", files: originals }))
  const before = store.getState().conversation
  store.dispatch(
    attachFiles({
      conversationId: "c0",
      files: originals.map((resource) => ({ ...resource, size: 1 })),
    }),
  )
  expect(store.getState().conversation).toBe(before)
  for (const resource of originals) {
    const minted = AttachmentResource.create({ ...resource, size: 1 })
    expect(dependencies.attachments.bytes(minted)).toBeUndefined()
    expect(dependencies.attachments.bytes({ ...resource, size: 1 })).toBeUndefined()
    store.dispatch(attachFiles({ conversationId: "c0", files: [resource] }))
    const placed = store.getState().conversation.conversations[0]!.draft[0]!
    if (placed.type !== "file") throw new Error("Resource was not admitted")
    expect(placed.resource).toBe(resource)
    store.dispatch(
      uploadChanged({
        conversationId: "c0",
        placementId: placed.placementId,
        to: "uploading",
      }),
    )
    await store.dispatch(
      stageAttachment({
        id: "c0",
        placementId: placed.placementId,
        file: {
          digest: `sha256:${"ab".repeat(32)}`,
          mimeType: resource.mimeType,
          size: resource.size,
        },
        bytes: dependencies.attachments.bytes(resource)!,
      }),
    )
    await store.dispatch(followConversation("c0"))
    const sent = await store.dispatch(sendDraft({ id: "c0", content: [] }))
    expect(sent.meta.requestStatus).toBe("fulfilled")
    const turn = store
      .getState()
      .conversation.conversations[0]!.turns.filter((turn) => turn.from === "user")
      .at(-1)!
    if (turn.from !== "user") throw new Error("Expected local turn")
    expect(turn.content.find((part) => part.type === "file")?.resource).toBe(resource)
    expect(dependencies.attachments.bytes(resource)?.size).toBe(size)
  }
  expect(attachmentResourceUsage(store.getState().conversation).needed.size).toBe(2)
  store.dispatch(closeConversation("c1"))
  const usage = attachmentResourceUsage(store.getState().conversation)
  expect(usage.needed.size).toBe(0)
  expect([...usage.previews].reduce((total, resource) => total + resource.size, 0)).toBe(
    size,
  )
  expect(
    originals.reduce(
      (total, resource) => total + (dependencies.attachments.bytes(resource)?.size ?? 0),
      0,
    ),
  ).toBe(size)
})

it("AL19/AL21: resource references survive refused recovery and Redux still rejects unrelated nonserializable data", async () => {
  vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:recovered")
  vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const diagnostic = vi.spyOn(console, "error").mockImplementation(() => {})
  const dependencies = createDependencies({
    conversation: {
      ...scenarioEffects("echo"),
      submit: async () => {
        throw new SubmissionRefusedError("attachment-not-found")
      },
    },
  })
  const store = makeStore(dependencies)
  const [resource] = dependencies.attachments.add([
    new File(["abc"], "image.png", { type: "image/png" }),
  ])
  store.dispatch(attachFiles({ conversationId: "c0", files: [resource!] }))
  store.dispatch(
    uploadChanged({ conversationId: "c0", placementId: "a1", to: "uploading" }),
  )
  await store.dispatch(
    stageAttachment({
      id: "c0",
      placementId: "a1",
      file: {
        digest: `sha256:${"ab".repeat(32)}`,
        mimeType: resource!.mimeType,
        size: resource!.size,
      },
      bytes: dependencies.attachments.bytes(resource!)!,
    }),
  )
  await store.dispatch(followConversation("c0"))
  const result = await store.dispatch(sendDraft({ id: "c0", content: [] }))
  expect(result.meta.requestStatus).toBe("rejected")
  expect(
    store
      .getState()
      .conversation.conversations[0]!.turns.some(
        (turn) => turn.from === "user" && turn.receipt === "failed",
      ),
  ).toBe(true)
  const original = { resource: resource! }
  const same = createNextState(original, (draft) => {
    expect(Reflect.set(draft.resource, "size", 1)).toBe(false)
  })
  expect(same.resource).toBe(resource)
  expect(same.resource.size).toBe(3)
  const recovered = store.getState().conversation.conversations[0]!.draft[0]!
  if (recovered.type !== "file") throw new Error("Expected recovered placement")
  expect(recovered.resource).toBe(resource)
  expect(dependencies.attachments.bytes(resource!)?.size).toBe(3)
  expect(diagnostic).not.toHaveBeenCalled()
  store.dispatch({ type: "probe/metadata", payload: { nested: [resource!] } })
  expect(diagnostic).not.toHaveBeenCalled()
  store.dispatch({
    type: "probe/blob",
    payload: { resource, other: new Blob(["unrelated"]) },
  })
  expect(diagnostic).toHaveBeenCalledOnce()
})
