import { AttachmentResource } from "../../../../src/conversation/testing"
/** Real App/store/editor composition for controlled native-read and URL-drop browser races. */
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { App } from "../../../../src/panel/ui/app"
import { ClockProvider } from "../../../../src/conversation/ui/clock"
import { createAttachmentResources } from "../../../../src/panel/adapters/attachment-resources"
import { makeStore } from "../../../../src/store"
import {
  attachFiles,
  bindConversation,
  closeConversation,
  scenarioEffects,
  setDraft,
  stageAttachment,
  uploadChanged,
  removeFile,
  openConversation,
} from "../../../../src/conversation/testing"
import { sessionReady } from "../../../../src/session/testing"
import "../../../../src/styles.css"

const png = Uint8Array.from(
  atob(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII=",
  ),
  (c) => c.charCodeAt(0),
)
let releaseRead: (value: ArrayBuffer) => void
const nativeRead = new Promise<ArrayBuffer>((resolve) => {
  releaseRead = resolve
})
let uploadReply:
  | {
      resolve: (value: { digest: string; mimeType: "image/png"; size: number }) => void
      reject: (cause: Error) => void
    }
  | undefined
let sharedReplies:
  | Map<
      string,
      {
        resolve: (image: { digest: string; mimeType: "image/png"; size: number }) => void
        aborted: boolean
      }
    >
  | undefined
let sharedResource: AttachmentResource | undefined
let uploadReached: (() => void) | undefined
let uploadResult: Promise<unknown> | undefined
const detachedReplies: Array<
  (image: { digest: string; mimeType: "image/png"; size: number }) => void
> = []
let detached = false
const sends: unknown[] = []
let fetchCalls = 0
const resources = createAttachmentResources()
const effects = scenarioEffects("echo")
const store = makeStore({
  attachments: resources,
  canChoosePaths: true,
  conversation: {
    ...effects,
    stageAttachment: (id, file, bytes, signal) => {
      if (detached) return new Promise((resolve) => detachedReplies.push(resolve))
      if (sharedReplies)
        return new Promise((resolve, reject) => {
          const reply = { resolve, aborted: false }
          sharedReplies!.set(id, reply)
          signal.addEventListener(
            "abort",
            () => {
              reply.aborted = true
              reject(new Error("placement removed"))
            },
            { once: true },
          )
        })
      return uploadReply
        ? new Promise((resolve, reject) => {
            uploadReply = { resolve, reject }
            uploadReached?.()
          })
        : effects.stageAttachment(id, file, bytes, signal)
    },
    submit: async (input) => {
      sends.push(input)
      return effects.submit(input)
    },
  },
})
let displayStore = store
let replacementStore = store
let replacementResources = resources
store.dispatch(
  sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
)
store.dispatch(setDraft({ draft: [{ type: "text", text: "include this image" }] }))
const realFetch = window.fetch.bind(window)
window.fetch = (input, init) => {
  if (
    typeof input !== "string" ||
    !input.startsWith("https://attachment-verification.invalid/")
  )
    return realFetch(input, init)
  fetchCalls++
  return new Promise<Response>((_resolve, reject) => {
    init?.signal?.addEventListener("abort", () => reject(init.signal?.reason), {
      once: true,
    })
  })
}
type PickerFile = {
  path: string
  name: string
  size: number
  mimeType: string
  ticket: string
}
type PickerReadyEvent = { id: string; name: string; readying: boolean }
const probe = {
  drop: undefined as undefined | ((payload: unknown) => void),
  began: undefined as
    undefined | ((batch: string, gesture: "picked" | "dropped") => void),
  readied: undefined as undefined | ((file: PickerReadyEvent) => void),
  pickRequests: [] as { requestId: string; resolve: (files: PickerFile[]) => void }[],
  holdPickers: false,
  choose: (requestId: string) =>
    probe.holdPickers
      ? new Promise<PickerFile[]>((resolve) =>
          probe.pickRequests.push({ requestId, resolve }),
        )
      : Promise.resolve([
          {
            path: "/verification/picked.png",
            name: "picked.png",
            size: png.length,
            mimeType: "image/png",
            ticket: "verification-native-ticket",
          },
        ]),
  announcePick: (requestId: string, name: string) => {
    probe.began?.(requestId, "picked")
    probe.readied?.({ id: `${requestId}:0`, name, readying: true })
  },
  settlePick: (requestId: string, files: PickerFile[]) => {
    const request = probe.pickRequests.find((pending) => pending.requestId === requestId)
    if (!request) throw new Error(`Unknown picker request ${requestId}`)
    probe.readied?.({ id: `${requestId}:0`, name: files[0]?.name ?? "", readying: false })
    request.resolve(files)
  },
  read: async (ticket: string) => {
    if (ticket !== "verification-native-ticket")
      throw new Error("Unexpected native attachment ticket")
    return nativeRead
  },
  completeRead: () => releaseRead(png.buffer),
  startParentUpload: async (sameParent = false) => {
    const reached = new Promise<void>((resolve) => {
      uploadReached = resolve
    })
    uploadReply = { resolve: () => {}, reject: () => {} }
    const file = AttachmentResource.create({
      name: "race.png",
      mimeType: "image/png",
      size: png.length,
      previewUrl: "",
      path: null,
    })
    store.dispatch(
      bindConversation({ id: "c0", serverId: "00000000-0000-4000-8000-000000000001" }),
    )
    store.dispatch(attachFiles({ conversationId: "c0", files: [file] }))
    store.dispatch(
      uploadChanged({ conversationId: "c0", placementId: "a1", to: "uploading" }),
    )
    uploadResult = store.dispatch(
      stageAttachment({
        id: "c0",
        placementId: "a1",
        file: {
          digest: `sha256:${"ab".repeat(32)}`,
          mimeType: file.mimeType,
          size: file.size,
        },
        bytes: new Blob([png]),
      }),
    )
    await reached
    if (sameParent) store.dispatch(removeFile("a1"))
    else store.dispatch(closeConversation("c0"))
    const id = store.getState().conversation.activeId
    store.dispatch(attachFiles({ conversationId: id, files: [file] }))
    store.dispatch(
      uploadChanged({ conversationId: id, placementId: "a2", to: "uploading" }),
    )
  },
  finishParentUpload: async (failed: boolean) => {
    if (failed) uploadReply!.reject(new Error("Old upload failed"))
    else
      uploadReply!.resolve({
        digest: `sha256:${"ef".repeat(32)}`,
        mimeType: "image/png",
        size: png.length,
      })
    await uploadResult
  },
  startSharedUploads: () => {
    sharedReplies = new Map()
    store.dispatch(
      bindConversation({ id: "c0", serverId: "00000000-0000-4000-8000-000000000001" }),
    )
    const [resource] = resources.add([
      new File([png], "shared.png", { type: "image/png" }),
    ])
    sharedResource = resource!

    store.dispatch(attachFiles({ conversationId: "c0", files: [resource!] }))
    store.dispatch(openConversation())
    store.dispatch(
      bindConversation({ id: "c1", serverId: "00000000-0000-4000-8000-000000000002" }),
    )
    store.dispatch(attachFiles({ conversationId: "c1", files: [resource!] }))
  },
  removeSharedParent: () => store.dispatch(closeConversation("c0")),
  finishSharedUpload: () =>
    sharedReplies!.get("00000000-0000-4000-8000-000000000002")!.resolve({
      digest: `sha256:${"ef".repeat(32)}`,
      mimeType: "image/png",
      size: png.length,
    }),
  closeSharedParent: () => store.dispatch(closeConversation("c1")),
  attachNativeSvg: () => {
    const [resource] = resources.add([
      new File(
        [
          '<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="red"/></svg>',
        ],
        "native.svg",
        { type: "image/svg+xml" },
      ),
    ])
    store.dispatch(attachFiles({ conversationId: "c0", files: [resource!] }))
  },
  startDetachedUploads: () => {
    detached = true
    store.dispatch(
      bindConversation({ id: "c0", serverId: "00000000-0000-4000-8000-000000000001" }),
    )
    const files = resources.add(
      Array.from(
        { length: 4 },
        (_, index) => new File([png], `${index}.png`, { type: "image/png" }),
      ),
    )
    store.dispatch(attachFiles({ conversationId: "c0", files }))
  },
  detachCoordinator: () => application.render(null),
  remountCoordinator: () => showScope(store, resources),
  finishDetachedUpload: () =>
    detachedReplies[0]!({
      digest: `sha256:${"ef".repeat(32)}`,
      mimeType: "image/png",
      size: png.length,
    }),
  startOwnerUpload: () => {
    sharedReplies = new Map()
    store.dispatch(
      bindConversation({ id: "c0", serverId: "00000000-0000-4000-8000-000000000001" }),
    )
    const [resource] = resources.add([
      new File([png], "original.png", { type: "image/png" }),
    ])
    store.dispatch(attachFiles({ conversationId: "c0", files: [resource!] }))
  },
  swapOwner: () => {
    const nextResources = createAttachmentResources()
    const nextEffects = scenarioEffects("echo")
    const nextStore = makeStore({
      attachments: nextResources,
      canChoosePaths: true,
      conversation: {
        ...nextEffects,
        stageAttachment: (id, _file, _bytes, signal) =>
          new Promise((resolve, reject) => {
            const reply = { resolve, aborted: false }
            sharedReplies!.set(id, reply)
            signal.addEventListener(
              "abort",
              () => {
                reply.aborted = true
                reject(new Error("placement removed"))
              },
              { once: true },
            )
          }),
      },
    })
    nextStore.dispatch(
      sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
    )
    nextStore.dispatch(
      bindConversation({ id: "c0", serverId: "00000000-0000-4000-8000-000000000002" }),
    )
    const [resource] = nextResources.add([
      new File([png], "replacement.png", { type: "image/png" }),
    ])
    nextStore.dispatch(attachFiles({ conversationId: "c0", files: [resource!] }))
    replacementStore = nextStore
    replacementResources = nextResources
    displayStore = nextStore
    showScope(nextStore, nextResources)
  },
  showOriginalOwner: () => {
    displayStore = store
    showScope(store, resources)
  },
  showReplacementOwner: () => {
    displayStore = replacementStore
    showScope(replacementStore, replacementResources)
  },
  finishOriginalOwner: () =>
    sharedReplies!.get("00000000-0000-4000-8000-000000000001")!.resolve({
      digest: `sha256:${"ef".repeat(32)}`,
      mimeType: "image/png",
      size: png.length,
    }),
  snapshot: () => ({
    detachedStageCount: detachedReplies.length,
    sharedUploads: sharedReplies
      ? [...sharedReplies].map(([id, reply]) => ({ id, aborted: reply.aborted }))
      : [],
    sharedBytesRetained: sharedResource
      ? resources.bytes(sharedResource) !== undefined
      : false,
    tabs: displayStore.getState().conversation.conversations.map((tab) => ({
      id: tab.id,
      files: tab.draft
        .filter((part) => part.type === "file")
        .map((part) => ({ ...part, sharedResource: part.resource === sharedResource })),
    })),
    sends,
    fetchCalls,
    draft: displayStore
      .getState()
      .conversation.conversations.find(
        (c) => c.id === displayStore.getState().conversation.activeId,
      )?.draft,
    pickerRequestIds: probe.pickRequests.map((request) => request.requestId),
  }),
}
;(window as unknown as { __attachmentProbe: typeof probe }).__attachmentProbe = probe
const root = document.getElementById("root")
if (!root) throw new Error("Missing attachment verification root")
const application = createRoot(root)
function showScope(
  scopeStore: ReturnType<typeof makeStore>,
  scopeResources: ReturnType<typeof createAttachmentResources>,
) {
  application.render(
    <Provider store={scopeStore}>
      {/* A fixture is its own composition root, so it chooses the clock. */}
      <ClockProvider now={() => Date.now()}>
        <App
          attachmentResources={scopeResources}
          canChoosePaths
          digest={async () => `sha256:${"ab".repeat(32)}`}
          followConversationChoices={(show) => {
            show({
              catalog: {
                agents: [],
                environments: [],
                commandHosts: [],
                nativeHooks: { claude: "off", codex: "off" },
              },
              chosenAgent: undefined,
            })
            return () => undefined
          }}
        />
      </ClockProvider>
    </Provider>,
  )
}
showScope(store, resources)
