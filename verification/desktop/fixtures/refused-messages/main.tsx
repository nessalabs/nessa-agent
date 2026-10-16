/**
 * The panel's real refusal notice over a store with two refused messages, the
 * newer one sent again and accepted. Send Again on the older one must send it.
 * With `?case=image` the older message carries an image: Send Again puts it
 * back to upload again, and `window.__upload` steps that upload.
 */
import { createElement } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { createDependencies } from "../../../../src/composition/dependencies"
import { makeStore } from "../../../../src/store"
import {
  attachFiles,
  scenarioEffects,
  sendDraft,
  setDraft,
  uploadChanged,
} from "../../../../src/conversation/testing"
import { AttachmentResource, textContent } from "../../../../src/conversation/model"
import { ConversationNotification } from "../../../../src/conversation/ui/conversation-notification"
import "../../../../src/styles.css"

const withImage = new URLSearchParams(location.search).get("case") === "image"
const effects = scenarioEffects("echo")
const state = { armed: !withImage, refusals: 2, submitted: [] as string[] }
let release = () => {}
const typed = new Promise<void>((resolve) => {
  release = resolve
})
Object.assign(window, { __refused: state })
const store = makeStore(
  createDependencies({
    conversation: {
      ...effects,
      create: async (...args: Parameters<typeof effects.create>) => {
        if (state.armed) {
          await typed
          if (state.refusals-- > 0) throw new Error("offline")
        }
        return effects.create(...args)
      },
      submit: async (input: Parameters<typeof effects.submit>[0]) => {
        state.submitted.push(input.text)
        return effects.submit(input)
      },
    },
  }),
)
const tab = () => {
  const found = store.getState().conversation.conversations[0]
  if (!found) throw new Error("missing conversation")
  return found
}
const id = tab().id
const image = {
  digest: `sha256:${"ab".repeat(32)}`,
  mimeType: "image/png" as const,
  size: 3,
}
const upload = (to: "not-started" | "uploading" | "stored" | "failed") => {
  const placementId = tab().draft.find((part) => part.type === "file")?.placementId
  if (!placementId) throw new Error("no image in the draft")
  store.dispatch(
    to === "stored"
      ? uploadChanged({ conversationId: id, placementId, to, image })
      : to === "failed"
        ? uploadChanged({ conversationId: id, placementId, to, reason: "unavailable" })
        : uploadChanged({ conversationId: id, placementId, to }),
  )
}
Object.assign(window, { __upload: upload })
let first = textContent("first")
if (withImage) {
  // A message sent first, so the conversation knows its agent takes images.
  await store.dispatch(sendDraft({ id, content: textContent("hello") }))
  const resource = AttachmentResource.create({
    name: "first.png",
    mimeType: "image/png",
    size: 3,
    previewUrl: "",
    path: null,
  })
  store.dispatch(setDraft({ id, draft: textContent("first") }))
  store.dispatch(attachFiles({ conversationId: id, files: [resource] }))
  upload("uploading")
  upload("stored")
  first = tab().draft
  state.armed = true
}
// The older message is refused while the newer one is already being typed.
const sending = store.dispatch(sendDraft({ id, content: first }))
store.dispatch(setDraft({ id, draft: textContent("second") }))
release()
await sending
await store.dispatch(sendDraft({ id, content: textContent("second") }))
const newer = tab().turns.at(-1)
if (!newer) throw new Error("missing newer refusal")
await store.dispatch(
  sendDraft({ id, content: textContent("second"), retryTurnId: newer.id }),
)

const root = document.getElementById("root")
if (!root) throw new Error("missing root")
root.dataset.surface = "panel"
const view = createRoot(root)
const render = () =>
  view.render(
    createElement(Provider, {
      store,
      children: createElement(
        "main",
        { style: { padding: 16 } },
        createElement(ConversationNotification, {
          conversation: tab(),
          connection: { phase: "ready", detail: "", retry: () => {} },
          gatewayAvailable: true,
        }),
        createElement("output", {
          "data-turns": tab()
            .turns.map((turn) => (turn.from === "user" ? turn.receipt : turn.from))
            .join(","),
        }),
      ),
    }),
  )
store.subscribe(render)
render()
