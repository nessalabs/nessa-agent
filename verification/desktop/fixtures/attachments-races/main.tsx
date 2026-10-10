/** Real App/store/editor composition for controlled native-read and URL-drop browser races. */
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { App } from "../../../../src/panel/ui/app"
import { createAttachmentResources } from "../../../../src/panel/adapters/attachment-resources"
import { makeStore } from "../../../../src/store"
import { scenarioEffects, setDraft } from "../../../../src/conversation/testing"
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
const sends: unknown[] = []
let fetchCalls = 0
const resources = createAttachmentResources()
const effects = scenarioEffects("echo")
const store = makeStore({
  attachments: resources,
  canChoosePaths: true,
  conversation: {
    ...effects,
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
    <App
      attachmentResources={resources}
      canChoosePaths
      digest={async () => `sha256:${"ab".repeat(32)}`}
      loadConversationChoices={async () => ({
        catalog: { agents: [], environments: [] },
        chosenAgent: undefined,
      })}
    />
  </Provider>,
)
