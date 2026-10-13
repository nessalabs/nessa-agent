import { expect, it } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { scenarioEffects } from "../scenario/effects"
import { AttachmentStagingError, type ConversationEffects } from "../../application/ports"
import type { FileAttachment } from "../../model"
import {
  attachFiles,
  bindConversation,
  closeConversation,
  openConversation,
  removeFile,
  setActive,
  stageAttachment,
  uploadChanged,
} from "./slice"

const first = "00000000-0000-4000-8000-000000000001"
const second = "00000000-0000-4000-8000-000000000002"
const stored = {
  digest: `sha256:${"ef".repeat(32)}`,
  mimeType: "image/jpeg" as const,
  size: 2,
}
const file: FileAttachment = {
  type: "file",
  resourceId: "same-file",
  placementId: "same-file",
  name: "image.jpg",
  mimeType: "image/jpeg",
  size: 3,
  previewUrl: "",
  path: null,
  upload: { status: "not-started" },
}
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}
function fixture(overrides: Partial<ConversationEffects> = {}) {
  const store = makeStore(
    createDependencies({ conversation: { ...scenarioEffects("echo"), ...overrides } }),
  )
  store.dispatch(bindConversation({ id: "c0", serverId: first }))
  store.dispatch(attachFiles({ conversationId: "c0", files: [file] }))
  store.dispatch(
    uploadChanged({ conversationId: "c0", placementId: "a1", to: "uploading" }),
  )
  store.dispatch(openConversation())
  store.dispatch(bindConversation({ id: "c1", serverId: second }))
  store.dispatch(attachFiles({ conversationId: "c1", files: [file] }))
  store.dispatch(
    uploadChanged({ conversationId: "c1", placementId: "a2", to: "uploading" }),
  )
  return store
}
function upload(store: ReturnType<typeof makeStore>, id: string) {
  return store.dispatch(
    stageAttachment({
      id,
      placementId: id === "c0" ? "a1" : "a2",
      file: {
        digest: `sha256:${"ab".repeat(32)}`,
        mimeType: file.mimeType,
        size: file.size,
      },
      bytes: new Blob(["raw"]),
    }),
  )
}
function state(store: ReturnType<typeof makeStore>, id: string) {
  return store.getState().conversation.conversations.find((tab) => tab.id === id)
    ?.draft[0]
}

it.each(["success", "failure"] as const)(
  "UP1: %s for a second parent does not update the first parent's shared resource",
  async (outcome) => {
    const calls: string[] = []
    const store = fixture({
      stageAttachment: async (id) => {
        calls.push(id)
        if (outcome === "failure") throw new AttachmentStagingError("unsupported-image")
        return stored
      },
    })
    const before = state(store, "c0")
    await upload(store, "c1")
    expect(calls).toEqual([second])
    expect(state(store, "c0")).toBe(before)
    expect(state(store, "c1")).toMatchObject({
      upload:
        outcome === "success"
          ? { status: "stored", image: stored }
          : { status: "failed", reason: "unsupported-image" },
    })
  },
)

it.each(["preparation", "staging"] as const)(
  "UP3: late results during %s leave a replacement parent's shared resource unchanged",
  async (waiting) => {
    for (const outcome of ["success", "failure"] as const) {
      const blocked = deferred<void>()
      const reached = deferred<void>()
      const calls: string[] = []
      const store = fixture({
        create: async (id) => {
          if (waiting === "preparation") {
            reached.resolve()
            await blocked.promise
          }
          return { conversationId: id }
        },
        stageAttachment: async (id) => {
          calls.push(id)
          if (waiting === "staging") {
            reached.resolve()
            await blocked.promise
          }
          if (outcome === "failure") throw new AttachmentStagingError("interrupted")
          return stored
        },
      })
      const pending = upload(store, "c0")
      await reached.promise
      store.dispatch(closeConversation("c0"))
      const replacement = state(store, "c1")
      blocked.resolve()
      await pending
      expect(calls).toEqual([first])
      expect(state(store, "c1")).toBe(replacement)
    }
  },
)

it("UP4: an aborted upload cannot mark another parent's file failed", async () => {
  const reached = deferred<void>()
  const store = fixture({
    stageAttachment: async (_id, _file, _bytes, signal) => {
      reached.resolve()
      await new Promise<void>((_done, reject) =>
        signal.addEventListener(
          "abort",
          () => reject(new AttachmentStagingError("interrupted")),
          { once: true },
        ),
      )
      return stored
    },
  })
  const pending = upload(store, "c1")
  await reached.promise
  store.dispatch(closeConversation("c1"))
  const other = state(store, "c0")
  pending.abort()
  await pending
  // Wait for the real adapter rejection handler, not just the thunk's abort race.
  await Promise.resolve()
  await Promise.resolve()
  expect(state(store, "c0")).toBe(other)
})

it("UP4: a removed tile's response does not resurrect it or change another parent", async () => {
  const blocked = deferred<typeof stored>()
  const reached = deferred<void>()
  const store = fixture({
    stageAttachment: async () => {
      reached.resolve()
      return blocked.promise
    },
  })
  const pending = upload(store, "c1")
  await reached.promise
  store.dispatch(setActive("c1"))
  store.dispatch(removeFile("a2"))
  const other = state(store, "c0")
  blocked.resolve(stored)
  await pending
  expect(state(store, "c1")).toBeUndefined()
  expect(state(store, "c0")).toBe(other)
})

it("UP5: failure and retry retain the selected parent while the active tab changes", async () => {
  let attempts = 0
  const store = fixture({
    stageAttachment: async () => {
      if (attempts++ === 0) throw new AttachmentStagingError("busy")
      return stored
    },
  })
  const other = state(store, "c0")
  await upload(store, "c1")
  expect(state(store, "c1")).toMatchObject({
    upload: { status: "failed", reason: "busy" },
  })
  store.dispatch(
    uploadChanged({ conversationId: "c1", placementId: "a2", to: "not-started" }),
  )
  store.dispatch(
    uploadChanged({ conversationId: "c1", placementId: "a2", to: "uploading" }),
  )
  store.dispatch(setActive("c0"))
  await upload(store, "c1")
  expect(state(store, "c1")).toMatchObject({
    upload: { status: "stored", image: stored },
  })
  expect(state(store, "c0")).toBe(other)
})

it("UP3: editing a retained input while preparation waits cannot redirect its result", async () => {
  const blocked = deferred<void>()
  const reached = deferred<void>()
  const store = fixture({
    create: async (id) => {
      reached.resolve()
      await blocked.promise
      return { conversationId: id }
    },
    stageAttachment: async () => stored,
  })
  const input = {
    id: "c1",
    placementId: "a2",
    file: {
      digest: `sha256:${"ab".repeat(32)}`,
      mimeType: file.mimeType,
      size: file.size,
    },
    bytes: new Blob(["raw"]),
  }
  const pending = store.dispatch(stageAttachment(input))
  await reached.promise
  input.id = "c0"
  input.placementId = "another-file"
  const other = state(store, "c0")
  blocked.resolve()
  await pending
  expect(state(store, "c0")).toBe(other)
  expect(state(store, "c1")).toMatchObject({
    upload: { status: "stored", image: stored },
  })
})

it.each(["preparation", "staging"] as const)(
  "AL3: replacing a placement during %s rejects the old success/failure",
  async (waiting) => {
    for (const outcome of ["success", "failure"] as const) {
      const blocked = deferred<void>()
      const reached = deferred<void>()
      const store = fixture({
        create: async (id) => {
          if (waiting === "preparation") {
            reached.resolve()
            await blocked.promise
          }
          return { conversationId: id }
        },
        stageAttachment: async () => {
          if (waiting === "staging") {
            reached.resolve()
            await blocked.promise
          }
          if (outcome === "failure") throw new AttachmentStagingError("interrupted")
          return stored
        },
      })
      const pending = upload(store, "c1")
      await reached.promise
      store.dispatch(setActive("c1"))
      store.dispatch(removeFile("a2"))
      store.dispatch(attachFiles({ conversationId: "c1", files: [file] }))
      const replacement = state(store, "c1")
      if (!replacement || replacement.type !== "file")
        throw new Error("Attachment was not admitted")
      expect(replacement.placementId).not.toBe("a2")
      store.dispatch(
        uploadChanged({
          conversationId: "c1",
          placementId: replacement.placementId,
          to: "uploading",
        }),
      )
      const before = state(store, "c1")
      blocked.resolve()
      await pending
      expect(state(store, "c1")).toBe(before)
    }
  },
)
