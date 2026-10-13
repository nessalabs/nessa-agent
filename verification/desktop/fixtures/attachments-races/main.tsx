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
let uploadReached: (() => void) | undefined
let uploadResult: Promise<unknown> | undefined
const sends: unknown[] = []
let fetchCalls = 0
const resources = createAttachmentResources()
const effects = scenarioEffects("echo")
const store = makeStore({
  attachments: resources,
  canChoosePaths: true,
  conversation: {
    ...effects,
    stageAttachment: (id, file, bytes, signal) =>
      uploadReply
        ? new Promise((resolve, reject) => {
            uploadReply = { resolve, reject }
            uploadReached?.()
          })
        : effects.stageAttachment(id, file, bytes, signal),
    send: async (input) => {
      sends.push(input)
      return effects.send(input)
    },
  },
})
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
const probe = {
  drop: undefined as undefined | ((payload: unknown) => void),
  choose: async () => [
    {
      path: "/verification/picked.png",
      name: "picked.png",
      size: png.length,
      mimeType: "image/png",
      ticket: "verification-native-ticket",
    },
  ],
  read: async (ticket: string) => {
    if (ticket !== "verification-native-ticket")
      throw new Error("Unexpected native attachment ticket")
    return nativeRead
  },
  completeRead: () => releaseRead(png.buffer),
  startParentUpload: async () => {
    const reached = new Promise<void>((resolve) => {
      uploadReached = resolve
    })
    uploadReply = { resolve: () => {}, reject: () => {} }
    const file = {
      type: "file" as const,
      id: "same-file",
      name: "race.png",
      mimeType: "image/png",
      size: png.length,
      previewUrl: "",
      path: null,
      upload: { status: "not-started" as const },
    }
    store.dispatch(
      bindConversation({ id: "c0", serverId: "00000000-0000-4000-8000-000000000001" }),
    )
    store.dispatch(attachFiles({ conversationId: "c0", files: [file] }))
    store.dispatch(
      uploadChanged({ conversationId: "c0", fileId: file.id, to: "uploading" }),
    )
    uploadResult = store.dispatch(
      stageAttachment({
        id: "c0",
        fileId: file.id,
        file: {
          digest: `sha256:${"ab".repeat(32)}`,
          mimeType: file.mimeType,
          size: file.size,
        },
        bytes: new Blob([png]),
      }),
    )
    await reached
    store.dispatch(closeConversation("c0"))
    const id = store.getState().conversation.activeId
    store.dispatch(attachFiles({ conversationId: id, files: [file] }))
    store.dispatch(
      uploadChanged({ conversationId: id, fileId: file.id, to: "uploading" }),
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
  snapshot: () => ({
    sends,
    fetchCalls,
    draft: store
      .getState()
      .conversation.conversations.find(
        (c) => c.id === store.getState().conversation.activeId,
      )?.draft,
  }),
}
;(window as unknown as { __attachmentProbe: typeof probe }).__attachmentProbe = probe
const root = document.getElementById("root")
if (!root) throw new Error("Missing attachment verification root")
createRoot(root).render(
  <Provider store={store}>
    {/* A fixture is its own composition root, so it chooses the clock. */}
    <ClockProvider now={() => Date.now()}>
      <App
        attachmentResources={resources}
        canChoosePaths
        digest={async () => `sha256:${"ab".repeat(32)}`}
        followConversationChoices={(show) => {
          show({
            catalog: {
              agents: [],
              environments: [],
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
